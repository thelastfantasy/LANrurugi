//! Extraction of `.route(path, method_router)` calls from a router factory's body.

use syn::visit::Visit;
use syn::{Expr, Lit};

use super::{HttpMethod, RouteRecord};

/// Walk a router factory's block, appending one [`RouteRecord`] per registered method.
///
/// `file` is already repository-relative; `errors` collects every unresolvable registration so one
/// run reports all of them instead of stopping at the first.
pub fn collect_routes(
    block: &syn::Block,
    factory: &str,
    module: &str,
    rel: &std::path::Path,
    out: &mut Vec<RouteRecord>,
    errors: &mut Vec<String>,
) {
    let mut collector = RouteCollector {
        factory,
        module,
        file: rel.display().to_string(),
        out,
        errors,
    };
    collector.visit_block(block);
}

struct RouteCollector<'a> {
    factory: &'a str,
    module: &'a str,
    file: String,
    out: &'a mut Vec<RouteRecord>,
    errors: &'a mut Vec<String>,
}

impl<'ast> Visit<'ast> for RouteCollector<'_> {
    fn visit_expr_method_call(&mut self, node: &'ast syn::ExprMethodCall) {
        if node.method == "route" {
            self.handle_route_call(node);
        }
        syn::visit::visit_expr_method_call(self, node);
    }
}

impl RouteCollector<'_> {
    fn handle_route_call(&mut self, node: &syn::ExprMethodCall) {
        let line = node.method.span().start().line;
        let where_ = format!("{}:{}", self.file, line);

        let path_arg = node.args.first();
        let router_arg = node.args.iter().nth(1);
        let (Some(path_arg), Some(router_arg)) = (path_arg, router_arg) else {
            self.errors.push(format!(
                "{where_}: `.route()` needs a path and a method router"
            ));
            return;
        };

        let declared_path = match literal_str(path_arg) {
            Some(p) => p,
            None => {
                self.errors.push(format!(
                    "{where_}: route path is not a string literal, so its full path cannot be \
                     determined. Routes must be written as literals for the generated API \
                     reference to cover them."
                ));
                return;
            }
        };

        let mut methods: Vec<(HttpMethod, Option<String>)> = Vec::new();
        if let Some(message) = decode_method_router(router_arg, &mut methods) {
            self.errors.push(format!("{where_}: {message}"));
            return;
        }
        if methods.is_empty() {
            self.errors.push(format!(
                "{where_}: the method router for `{declared_path}` names no HTTP method this \
                 scanner understands (expected one of get/post/put/delete/patch/head/options)."
            ));
            return;
        }

        for (method, handler) in methods {
            self.out.push(RouteRecord {
                method,
                absolute_path: join_path(self.mount_prefix(), &declared_path),
                declared_path: declared_path.clone(),
                factory: self.factory.to_string(),
                handler,
                module: self.module.to_string(),
                file: self.file.clone(),
                line,
            });
        }
    }

    /// The prefix this factory's declared paths live under. The mount table is resolved separately
    /// and asserted against the discovered factories; here it only needs to answer "is this one of
    /// the two bare factories", so a wrong answer cannot pass silently — `mount::MountTable::resolve`
    /// turns any disagreement into a build failure.
    fn mount_prefix(&self) -> &'static str {
        if super::mount::is_bare_factory(self.factory) {
            ""
        } else {
            "/api"
        }
    }
}

fn join_path(prefix: &str, declared: &str) -> String {
    if prefix.is_empty() {
        declared.to_string()
    } else if declared == "/" {
        prefix.to_string()
    } else {
        format!("{prefix}{declared}")
    }
}

fn literal_str(expr: &Expr) -> Option<String> {
    match unparen(expr) {
        Expr::Lit(lit) => match &lit.lit {
            Lit::Str(s) => Some(s.value()),
            _ => None,
        },
        _ => None,
    }
}

fn unparen(expr: &Expr) -> &Expr {
    match expr {
        Expr::Paren(p) => unparen(&p.expr),
        Expr::Group(g) => unparen(&g.expr),
        other => other,
    }
}

/// Decode a method-router expression into the methods it registers.
///
/// Handles the bare form (`get(h)`), the crate-qualified form (`axum::routing::post(h)`), and
/// chains (`get(a).delete(b)`), plus pass-through combinators like `.layer(..)` that change no
/// route. Returns `Some(message)` when part of the expression cannot be interpreted.
fn decode_method_router(
    expr: &Expr,
    out: &mut Vec<(HttpMethod, Option<String>)>,
) -> Option<String> {
    match unparen(expr) {
        Expr::MethodCall(call) => {
            let method_name = call.method.to_string();
            if method_name == "any" {
                return Some(
                    "`any()` registers every HTTP method; this scanner does not model it. \
                     Register each method explicitly, or teach the scanner about `any()`."
                        .to_string(),
                );
            }
            // Recurse into the receiver first: `get(a).delete(b)` registers `get` before `delete`.
            if let Some(message) = decode_method_router(&call.receiver, out) {
                return Some(message);
            }
            match HttpMethod::parse(&method_name) {
                Some(method) => {
                    out.push((method, handler_name(call.args.first())));
                    None
                }
                // A combinator such as `.layer(..)`/`.route_layer(..)` adds no route; the methods
                // it wraps were already collected from the receiver.
                None => None,
            }
        }
        Expr::Call(call) => {
            let name = match &*call.func {
                Expr::Path(path) => path.path.segments.last()?.ident.to_string(),
                _ => String::new(),
            };
            if name == "any" {
                return Some(
                    "`any()` registers every HTTP method; this scanner does not model it. \
                     Register each method explicitly, or teach the scanner about `any()`."
                        .to_string(),
                );
            }
            match HttpMethod::parse(&name) {
                Some(method) => {
                    out.push((method, handler_name(call.args.first())));
                    None
                }
                None if name.is_empty() => Some(
                    "the method router is not a plain function call, so its HTTP methods cannot \
                     be determined."
                        .to_string(),
                ),
                None => Some(format!(
                    "`{name}(..)` is not an HTTP method this scanner understands."
                )),
            }
        }
        _ => Some(
            "the method router for this route is not an expression this scanner understands."
                .to_string(),
        ),
    }
}

fn handler_name(arg: Option<&Expr>) -> Option<String> {
    match arg.map(unparen)? {
        Expr::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string()),
        // A closure handler is real and legitimate; it simply has no name to report.
        _ => None,
    }
}
