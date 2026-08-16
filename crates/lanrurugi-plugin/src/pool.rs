//! Deno subprocess worker-pool manager (constitution Principle IV, research.md §7).
//!
//! Each plugin namespace gets its **own** persistent Deno subprocess, started with exactly that
//! plugin's declared permissions (never a shared process across plugins, which would otherwise
//! grant every plugin the union of every other enabled plugin's permissions — a Principle IV
//! violation). Starting a plugin's worker is therefore two-phase:
//! 1. Query `plugin_info` from a throwaway, **zero-permission** subprocess (the host can't know
//!    what to grant until it asks).
//! 2. Spawn the real, persistent worker with exactly the permissions that response declared.
//!
//! A failed or timed-out request kills and drops that one plugin's worker (it's respawned lazily
//! on next use) without touching any other in-flight request or plugin (FR-013 — failure
//! isolation).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

use crate::permissions::build_flags;
use crate::protocol::{PluginInfo, PluginOptionsResult, Request, Response};

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("failed to spawn Deno subprocess: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("plugin worker did not respond within {0:?} (timeout)")]
    Timeout(Duration),
    #[error("plugin returned an error: {}", .0.message)]
    PluginError(crate::protocol::ResponseError),
    #[error("plugin worker exited unexpectedly")]
    WorkerGone,
    #[error("malformed response from plugin worker: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("no plugin installed under namespace {0:?}")]
    NotFound(String),
}

type Result<T> = std::result::Result<T, PoolError>;

/// `namespace` ultimately gets `format!("{namespace}.ts")`-ed and joined onto `plugins_dir` to
/// locate the plugin's `.ts` file — but `plugin_info`/`execute` take it straight from an
/// unauthenticated-by-namespace HTTP query parameter (`POST /plugins/use`'s `plugin` param,
/// `crates/lanrurugi-api/src/plugins.rs`), not from `discover_namespaces`'s own directory listing.
/// Without this check, `PathBuf::join` treats a leading `/` in `namespace` as an **absolute**
/// path and discards `plugins_dir` entirely (Rust std behavior), and `..` components climb out of
/// it — either way letting a caller point the host at an arbitrary `.ts` file on disk, which would
/// then run with whatever permissions *that file's own* `plugin_info` response declares.
///
/// Namespaces may now contain subdirectory components (`metadata/ehentai`, `custom/foo/bar`) —
/// plugins are organized under `plugins_dir` by category (`metadata/`, `login/`, `download/`,
/// `script/`) plus a `custom/` tree for uploaded ones — so this only rejects the two genuinely
/// unsafe shapes (absolute paths, `..` traversal), not multi-component paths. Every component
/// must still be `Normal` (a plain name, not `.`/`..`/a root), which is exactly what
/// `discover_namespaces`'s own recursive walk (`file_stem()`-based, relative to `plugins_dir`)
/// always produces.
fn is_safe_namespace(namespace: &str) -> bool {
    !namespace.is_empty()
        && Path::new(namespace)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

struct Worker {
    stdin: ChildStdin,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>>,
    _child: Child,
}

impl Worker {
    async fn spawn(
        deno_binary: &str,
        dispatcher_path: &Path,
        plugins_dir: &Path,
        namespace: &str,
        extra_flags: &[String],
        declared_read: bool,
    ) -> Result<Self> {
        let plugin_module = plugins_dir.join(format!("{namespace}.ts"));
        // `plugin-sdk.ts` is written out as `dispatcher_path`'s own sibling (every
        // `lanrurugi_plugin::DISPATCHER_SCRIPT` write site has a matching
        // `lanrurugi_plugin::PLUGIN_SDK_SCRIPT` write right next to it — see those sites' own
        // comments), so this is a mechanical path join, not a lookup of anything Deno-side.
        let plugin_sdk_path = dispatcher_path
            .parent()
            .expect("dispatcher_path always has a parent directory")
            .join("plugin-sdk.ts");
        // Deno must read the dispatcher + SDK + plugin `.ts` files to `import()` them at all — a
        // mechanical requirement, not a capability grant — so this baseline is always present,
        // scoped to just those files unless the plugin itself declared broader read access.
        // `plugin-sdk.ts` is read by `dispatcher.ts` itself (a real `import` — see that file's own
        // top-of-file comment for why `PluginErrorException` needs one there but a plugin file
        // itself does not), so this worker process — which *is* the dispatcher process — needs the
        // grant even though the plugin module it later `import()`s never touches that path itself.
        let read_flag = if declared_read {
            "--allow-read".to_string()
        } else {
            format!(
                "--allow-read={},{},{}",
                dispatcher_path.display(),
                plugin_sdk_path.display(),
                plugin_module.display()
            )
        };

        let mut cmd = tokio::process::Command::new(deno_binary);
        cmd.arg("run").arg("--quiet").arg(read_flag);
        for flag in extra_flags {
            cmd.arg(flag);
        }
        cmd.arg(dispatcher_path)
            .arg(plugins_dir)
            .arg(namespace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");

        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_for_reader = pending.clone();

        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(response) = serde_json::from_str::<Response>(&line) else {
                    continue;
                };
                if let Some(sender) = pending_for_reader.lock().await.remove(&response.request_id) {
                    let _ = sender.send(response);
                }
            }
            // Reader loop ended (process exited): wake any still-pending callers with nothing —
            // their `oneshot::Receiver` will just observe the sender dropped.
        });

        Ok(Self {
            stdin,
            pending,
            _child: child,
        })
    }

    async fn call(
        &mut self,
        plugin: &str,
        method: &str,
        args: serde_json::Value,
    ) -> Result<Response> {
        let request_id = Uuid::new_v4().to_string();
        let request = Request {
            request_id: request_id.clone(),
            plugin: plugin.to_string(),
            method: method.to_string(),
            args,
        };

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await?;

        match tokio::time::timeout(DEFAULT_TIMEOUT, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(PoolError::WorkerGone),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err(PoolError::Timeout(DEFAULT_TIMEOUT))
            }
        }
    }
}

pub struct PluginPool {
    deno_binary: String,
    dispatcher_path: PathBuf,
    plugins_dir: PathBuf,
    workers: Arc<Mutex<HashMap<String, Worker>>>,
}

impl PluginPool {
    pub fn new(
        deno_binary: impl Into<String>,
        dispatcher_path: PathBuf,
        plugins_dir: PathBuf,
    ) -> Self {
        Self {
            deno_binary: deno_binary.into(),
            dispatcher_path,
            plugins_dir,
            workers: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Queries a plugin's declared metadata/permissions via a throwaway, zero-permission
    /// subprocess — never the persistent worker (which doesn't exist yet at this point).
    pub async fn plugin_info(&self, namespace: &str) -> Result<PluginInfo> {
        if !is_safe_namespace(namespace) {
            return Err(PoolError::NotFound(namespace.to_string()));
        }
        let plugin_module = self.plugins_dir.join(format!("{namespace}.ts"));
        if !plugin_module.is_file() {
            return Err(PoolError::NotFound(namespace.to_string()));
        }
        let mut worker = Worker::spawn(
            &self.deno_binary,
            &self.dispatcher_path,
            &self.plugins_dir,
            namespace,
            &[],
            false,
        )
        .await?;
        let response = worker
            .call(namespace, "plugin_info", serde_json::json!({}))
            .await?;
        response_to_result(response).and_then(|v| Ok(serde_json::from_value(v)?))
    }

    /// Queries a download plugin's optional `pluginOptions()` declaration via the same kind of
    /// throwaway, zero-permission subprocess as [`plugin_info`](Self::plugin_info) — cheap,
    /// side-effect-free, safe to call on every settings-page load. Returns `Ok(None)` (not an
    /// error) when the plugin exports no `pluginOptions()` at all (spec FR-015), which the
    /// dispatcher signals by returning `null`.
    pub async fn plugin_options(&self, namespace: &str) -> Result<Option<PluginOptionsResult>> {
        if !is_safe_namespace(namespace) {
            return Err(PoolError::NotFound(namespace.to_string()));
        }
        let plugin_module = self.plugins_dir.join(format!("{namespace}.ts"));
        if !plugin_module.is_file() {
            return Err(PoolError::NotFound(namespace.to_string()));
        }
        let mut worker = Worker::spawn(
            &self.deno_binary,
            &self.dispatcher_path,
            &self.plugins_dir,
            namespace,
            &[],
            false,
        )
        .await?;
        let response = worker
            .call(namespace, "plugin_options", serde_json::json!({}))
            .await?;
        let value = response_to_result(response)?;
        if value.is_null() {
            Ok(None)
        } else {
            Ok(Some(serde_json::from_value(value)?))
        }
    }

    /// Executes `method` against `namespace`'s persistent worker, starting it (with exactly its
    /// declared permissions) on first use.
    pub async fn execute(
        &self,
        namespace: &str,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if !is_safe_namespace(namespace) {
            return Err(PoolError::NotFound(namespace.to_string()));
        }
        let mut workers = self.workers.lock().await;
        if !workers.contains_key(namespace) {
            let info = self.plugin_info(namespace).await?;
            let flags = build_flags(&info.declared_permissions);
            let worker = Worker::spawn(
                &self.deno_binary,
                &self.dispatcher_path,
                &self.plugins_dir,
                namespace,
                &flags,
                info.declared_permissions.read,
            )
            .await?;
            workers.insert(namespace.to_string(), worker);
        }

        let worker = workers
            .get_mut(namespace)
            .expect("just inserted or present");
        let result = worker.call(namespace, method, args).await;

        // Failure isolation: drop this plugin's worker on any error so the next call gets a
        // fresh process, without touching any other plugin's worker or in-flight request.
        if result.is_err() {
            workers.remove(namespace);
        }

        response_to_result(result?)
    }
}

fn response_to_result(response: Response) -> Result<serde_json::Value> {
    if response.ok {
        Ok(response.result.unwrap_or(serde_json::Value::Null))
    } else {
        let error = response.error.unwrap_or(crate::protocol::ResponseError {
            message: "unknown error".to_string(),
            kind: "plugin_error".to_string(),
            error_code: None,
            data: None,
        });
        Err(PoolError::PluginError(error))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples_and_dispatcher() -> Option<(PathBuf, PathBuf)> {
        which_deno()?;
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let dispatcher = PathBuf::from(manifest_dir).join("dispatcher/dispatcher.ts");
        let samples = PathBuf::from(manifest_dir).join("samples");
        Some((dispatcher, samples))
    }

    fn which_deno() -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("deno"))
                .find(|p| p.is_file())
        })
    }

    #[test]
    fn is_safe_namespace_accepts_plain_identifiers() {
        assert!(is_safe_namespace("sample-metadata-plugin"));
        assert!(is_safe_namespace("plugin_123"));
    }

    #[test]
    fn is_safe_namespace_rejects_path_traversal_and_absolute_paths() {
        // Regression guard: `PathBuf::join` treats a leading `/` as absolute (discarding the
        // base entirely) and resolves `..` upward — either would let an unvalidated namespace
        // from an HTTP query parameter (`POST /plugins/use?plugin=...`) point at an arbitrary
        // `.ts` file outside `plugins_dir`.
        assert!(!is_safe_namespace("../escape"));
        assert!(!is_safe_namespace("../../etc/passwd"));
        assert!(!is_safe_namespace("/etc/passwd"));
        assert!(!is_safe_namespace(".."));
        assert!(!is_safe_namespace("."));
        assert!(!is_safe_namespace(""));
    }

    #[test]
    fn is_safe_namespace_accepts_multi_component_category_paths() {
        // Namespaces may now contain subdirectory components (`metadata/ehentai`, `custom/foo`) —
        // plugins are organized under `plugins_dir` by category (see `discover_namespaces` in
        // `lanrurugi-api::plugins`) plus a `custom/` tree for uploaded ones. Every component here
        // is still a plain name (`Normal`), which is what makes this safe: `a/b` can only ever
        // resolve to a literal `plugins_dir/a/b.ts`, never escape it, unlike `../` or an absolute
        // path.
        assert!(is_safe_namespace("metadata/ehentai"));
        assert!(is_safe_namespace("custom/metadata/foo"));
    }

    #[tokio::test]
    async fn plugin_info_matches_sample_plugins_declaration() {
        let Some((dispatcher, samples)) = samples_and_dispatcher() else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        let pool = PluginPool::new("deno", dispatcher, samples);
        let info = pool.plugin_info("sample-metadata-plugin").await.unwrap();
        assert_eq!(info.namespace, "sample-metadata-plugin");
        assert_eq!(info.kind, "metadata");
        assert_eq!(
            info.declared_permissions.net,
            vec!["metadata.example.invalid"]
        );
        assert!(!info.declared_permissions.read);
        assert!(!info.declared_permissions.write);
    }

    #[tokio::test]
    async fn plugin_options_returns_none_when_the_plugin_exports_none() {
        let Some((dispatcher, samples)) = samples_and_dispatcher() else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        let pool = PluginPool::new("deno", dispatcher, samples);
        let options = pool.plugin_options("sample-metadata-plugin").await.unwrap();
        assert!(options.is_none());
    }

    #[tokio::test]
    async fn plugin_options_matches_sample_download_plugins_declaration() {
        let Some((dispatcher, samples)) = samples_and_dispatcher() else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        let pool = PluginPool::new("deno", dispatcher, samples);
        let options = pool
            .plugin_options("sample-download-plugin")
            .await
            .unwrap()
            .expect("sample-download-plugin declares pluginOptions()");
        assert_eq!(options.domain_rules.len(), 1);
        assert_eq!(
            options.domain_rules[0].pattern.as_deref(),
            Some("*.download.example.invalid")
        );
        assert_eq!(options.domain_rules[0].max_concurrent, Some(2));
        let bundle = options
            .bundle_as_archive
            .expect("sample-download-plugin declares bundle_as_archive");
        assert!(bundle.default);
    }

    #[tokio::test]
    async fn exec_metadata_runs_end_to_end_through_the_real_dispatcher() {
        let Some((dispatcher, samples)) = samples_and_dispatcher() else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        let pool = PluginPool::new("deno", dispatcher, samples);
        let result = pool
            .execute(
                "sample-metadata-plugin",
                "exec_metadata",
                serde_json::json!({ "archive_id": "deadbeef" }),
            )
            .await
            .unwrap();
        assert_eq!(result["tags"], "source:sample,archive:deadbeef");
        assert_eq!(result["summary"], "Enriched by sample-metadata-plugin.");
    }

    #[tokio::test]
    async fn unknown_plugin_method_is_isolated_as_a_plugin_error() {
        let Some((dispatcher, samples)) = samples_and_dispatcher() else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        let pool = PluginPool::new("deno", dispatcher, samples);
        let err = pool
            .execute(
                "sample-metadata-plugin",
                "exec_download",
                serde_json::json!({}),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PoolError::PluginError(_)));

        // The pool itself must still be usable afterwards for the same plugin (failure isolation
        // doesn't poison future calls, just that one).
        let result = pool
            .execute(
                "sample-metadata-plugin",
                "exec_metadata",
                serde_json::json!({ "archive_id": "still-works" }),
            )
            .await
            .unwrap();
        assert_eq!(result["tags"], "source:sample,archive:still-works");
    }
}
