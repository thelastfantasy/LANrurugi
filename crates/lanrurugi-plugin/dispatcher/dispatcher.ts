// LANrurugi plugin dispatcher (constitution Principle IV, contracts/plugin-protocol.md).
//
// One dispatcher process is dedicated to exactly one plugin namespace (started with that
// plugin's own declared Deno permissions — see lanrurugi-plugin::pool). It reads
// newline-delimited JSON requests from stdin and dynamically import()s the plugin module on
// demand rather than being started fresh per call (research.md §7).
//
// Requests are dispatched without awaiting the previous one to finish, so multiple in-flight
// requests (e.g. metadata lookups for several archives at once) run concurrently within this one
// process — real concurrency for I/O-bound plugin work without needing multiple OS processes per
// plugin.
//
// No external module imports (not even Deno's own std streams helpers) so that a plugin's
// `--allow-net` grant is never implicitly widened by the dispatcher's own startup needs. Same
// reasoning is why `legacyCompat`/`PluginErrorException` below are plain global assignments rather
// than something a plugin would need to `import`: a plugin file lives under `plugins_dir`, which
// has no fixed relative path to this file's own directory (both are independent, runtime-supplied
// CLI args — `--plugins-dir`/`--temp-dir`) — a plugin file's own top-level `import` specifier has
// to be a static string, so it can't reach into a path it won't know until runtime. A global needs
// no such path and no extra `--allow-read` grant at all, since it's set in this same
// already-trusted process before the plugin module loads.
//
// `PluginErrorException` itself is `import`ed for real from `plugin-sdk.ts` right below (needing
// its own `--allow-read` grant — `lanrurugi_plugin::pool::Worker::spawn` grants `plugin-sdk.ts`'s
// path, written as `dispatcher_path`'s own sibling by every `DISPATCHER_SCRIPT` write site) since
// *this* import's specifier can be a static relative path (`dispatcher.ts` and `plugin-sdk.ts`
// always share one directory, `temp_dir`) — only a *plugin* file's own import of it would have the
// unresolvable-path problem above. Re-exposed as a global (`globalThis.PluginErrorException`
// below) purely so every plugin file can `throw new PluginErrorException(...)` without its own
// import at all (issue #86) — every plugin file still gets the exact same class reference this
// process's `catch` below checks with `instanceof`, since both paths ultimately go through this
// one already-imported module.
//
// The `/// <reference types="../../../plugins/legacy-globals.d.ts" />` directive right below is a
// *type-checker-only* hint — it costs nothing at runtime (erased entirely by the time `deno run`
// executes this file, same as any other `.d.ts`-style ambient declaration). It's what lets
// `plugins/legacy-globals.d.ts` (the canonical home for `Legacy*`/`legacyCompat`/
// `PluginErrorException`'s own ambient types — see that file's own docs) actually type-check this
// file's `globalThis.legacyCompat = {...}`/`globalThis.PluginErrorException = ...` assignments
// below. Every plugin file under `plugins/` sees the same declarations with no reference line of
// its own at all (`plugins/tsconfig.json` makes that file ambiently visible project-wide) — this
// file needs its own explicit line only because it lives outside `plugins/`, so that project
// boundary doesn't cover it.
/// <reference types="../../../plugins/legacy-globals.d.ts" />
import { PluginErrorException } from "./plugin-sdk.ts";

interface PluginRequest {
  request_id: string;
  plugin: string;
  method: string;
  args: unknown;
}

// ── Minimal HTML/XML parser + CSS-subset selector matcher (Mojo::DOM shim) ───────────────────────
// No third-party parsing library, per this file's own "no external module imports" rule (see the
// top-of-file docs) — self-contained, sized to exactly the selector/method shapes the legacy
// plugin corpus exercises (verified against every `->at(...)`/`->find(...)` call site across
// `~/LANraragi/lib/LANraragi/Plugin/**/*.pm`), not a general HTML parser.

interface RawNode {
  tag: string;
  attrs: Record<string, string>;
  children: RawNode[];
  parent: RawNode | undefined;
  /** Direct text content belonging to this element (concatenation of its own immediate text-node
   * children only — real `Mojo::DOM`'s own `->text` is also "this element's own text, not its
   * descendants'", so this matches that, not a full innerText-style recursive collection). */
  ownText: string;
}

// Elements whose content is never markup (script/style bodies routinely contain `<`/`>`/`&` that
// aren't real tags/entities — e.g. `<script>if (a < b) {}</script>` — parsing them as child nodes
// would corrupt the tree). Verified relevant to the corpus: `EHentai.pm`'s cover-image lookup
// parses a page containing inline `<script>` blocks via `->at('script')` on the *whole* script
// text, never anything nested inside one.
const RAW_TEXT_TAGS = new Set(["script", "style"]);
// Void/self-closing HTML elements that never get a closing tag in real markup — without this,
// e.g. a stray `<img src="...">` would swallow every sibling after it as its own "children" while
// this parser waits for a `</img>` that will never come.
const VOID_TAGS = new Set([
  "area", "base", "br", "col", "embed", "hr", "img", "input",
  "link", "meta", "param", "source", "track", "wbr",
]);

function parseAttrs(attrString: string): Record<string, string> {
  const attrs: Record<string, string> = {};
  const attrRe = /([a-zA-Z_:][-a-zA-Z0-9_:.]*)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/g;
  let m: RegExpExecArray | null;
  while ((m = attrRe.exec(attrString)) !== null) {
    const [, name, dq, sq, bare] = m;
    attrs[name.toLowerCase()] = dq ?? sq ?? bare ?? "";
  }
  return attrs;
}

/** Parses `markup` into a lightweight tree. `xml` disables lowercasing tag names (real XML/custom
 * elements — e.g. ComicInfo.xml's `<Genre>` — are case-sensitive; traditional HTML tag names
 * aren't, and legacy plugin selectors written against scraped HTML rely on that, e.g. matching
 * `at('h1')` regardless of the source's actual tag casing). */
function parseMarkup(markup: string, xml: boolean): RawNode {
  const root: RawNode = { tag: "#root", attrs: {}, children: [], parent: undefined, ownText: "" };
  const stack: RawNode[] = [root];
  const tagRe = /<!--[\s\S]*?-->|<!\[CDATA\[[\s\S]*?\]\]>|<(\/?)([a-zA-Z][-a-zA-Z0-9:]*)((?:[^>"']|"[^"]*"|'[^']*')*)(\/?)>/g;
  let lastIndex = 0;
  let m: RegExpExecArray | null;

  const appendText = (text: string) => {
    if (!text) return;
    const top = stack[stack.length - 1];
    top.ownText += text;
  };

  while ((m = tagRe.exec(markup)) !== null) {
    appendText(markup.slice(lastIndex, m.index));
    lastIndex = tagRe.lastIndex;

    if (m[0].startsWith("<!--") || m[0].startsWith("<![CDATA[")) continue;

    const [, closing, rawTag, rawAttrs, selfClosing] = m;
    const tag = xml ? rawTag : rawTag.toLowerCase();

    if (closing) {
      // Pop back to (and including) the matching open tag, tolerating mismatched/unclosed tags in
      // real-world scraped HTML rather than throwing — anything left dangling just stays a child
      // of whatever's above it, which is a harmless-enough approximation for a `->at()`/`->find()`
      // read-only query tool (this never needs to serialize the tree back out).
      for (let i = stack.length - 1; i > 0; i--) {
        if (stack[i].tag === tag) {
          stack.length = i;
          break;
        }
      }
      continue;
    }

    const node: RawNode = {
      tag,
      attrs: parseAttrs(rawAttrs),
      children: [],
      parent: stack[stack.length - 1],
      ownText: "",
    };
    stack[stack.length - 1].children.push(node);

    const isVoid = Boolean(selfClosing) || (!xml && VOID_TAGS.has(tag));
    if (isVoid) continue;

    if (!xml && RAW_TEXT_TAGS.has(tag)) {
      // Consume everything up to the matching closing tag as one raw text blob, bypassing the
      // normal tag scanner entirely — a `<` inside a script body must never be mistaken for a
      // real element start.
      const closeRe = new RegExp(`</${tag}\\s*>`, "i");
      const rest = markup.slice(lastIndex);
      const closeMatch = closeRe.exec(rest);
      const rawBody = closeMatch ? rest.slice(0, closeMatch.index) : rest;
      node.ownText = rawBody;
      lastIndex += rawBody.length + (closeMatch ? closeMatch[0].length : 0);
      tagRe.lastIndex = lastIndex;
      continue;
    }

    stack.push(node);
  }
  appendText(markup.slice(lastIndex));
  return root;
}

/** A single simple selector's parsed shape — everything the legacy corpus's own `->at`/`->find`
 * calls use: an optional tag name, an optional `.class` or `#id`, and any number of
 * `[attr]`/`[attr=val]`/`[attr^=val]` attribute conditions. No descendant combinators, no
 * pseudo-classes, no comma-separated selector lists — none appear anywhere in that corpus. */
interface ParsedSelector {
  tag: string | undefined;
  className: string | undefined;
  id: string | undefined;
  attrConditions: Array<{ name: string; op: "=" | "^=" | "*="; value: string } | { name: string; op: "exists" }>;
}

function parseSelector(selector: string): ParsedSelector {
  const parsed: ParsedSelector = { tag: undefined, className: undefined, id: undefined, attrConditions: [] };
  // Peel off `[attr...]` conditions first (there can be several), then whatever's left is
  // `tag`/`.class`/`#id` (at most one of the latter two, per the corpus's own usage).
  const attrRe = /\[([-a-zA-Z0-9_:]+)(?:([~^*$|]?=)"([^"]*)"|([~^*$|]?=)'([^']*)')?\]/g;
  let rest = selector;
  let m: RegExpExecArray | null;
  while ((m = attrRe.exec(selector)) !== null) {
    const name = m[1];
    const op = m[2] ?? m[4];
    const value = m[3] ?? m[5];
    if (op === undefined) {
      parsed.attrConditions.push({ name, op: "exists" });
    } else if (op === "^=") {
      parsed.attrConditions.push({ name, op: "^=", value: value ?? "" });
    } else if (op === "*=") {
      parsed.attrConditions.push({ name, op: "*=", value: value ?? "" });
    } else {
      // `=`, and the other combinator forms (`~=`/`$=`/`|=`) this corpus never uses — treated as
      // plain equality, a safe enough approximation for a selector shape that doesn't appear.
      parsed.attrConditions.push({ name, op: "=", value: value ?? "" });
    }
    rest = rest.replace(m[0], "");
  }
  const classMatch = rest.match(/\.([-a-zA-Z0-9_]+)/);
  if (classMatch) {
    parsed.className = classMatch[1];
    rest = rest.replace(classMatch[0], "");
  }
  const idMatch = rest.match(/#([-a-zA-Z0-9_]+)/);
  if (idMatch) {
    parsed.id = idMatch[1];
    rest = rest.replace(idMatch[0], "");
  }
  rest = rest.trim();
  if (rest) parsed.tag = rest;
  return parsed;
}

function nodeMatches(node: RawNode, selector: ParsedSelector, caseSensitive: boolean): boolean {
  if (selector.tag) {
    // HTML mode: both sides already lowercased is wrong to *assume* — the selector string itself
    // (e.g. a literal `"DIV"` in source) isn't normalized by the parser, only parsed *node* tag
    // names are (see `parseMarkup`'s `xml ? rawTag : rawTag.toLowerCase()`). So HTML mode compares
    // case-insensitively (both sides lowercased) same as real browsers/Mojo::DOM's own HTML mode;
    // XML mode compares the raw selector against the raw (case-preserved) parsed tag name exactly
    // — matching real XML's case-sensitive element names (this bug existed before: comparing both
    // sides lowercased *unconditionally* made every XML selector accidentally case-insensitive
    // too, e.g. incorrectly matching a `genre` selector against a parsed `<Genre>` element).
    const matches = caseSensitive
      ? node.tag === selector.tag
      : node.tag.toLowerCase() === selector.tag.toLowerCase();
    if (!matches) return false;
  }
  if (selector.id && node.attrs.id !== selector.id) return false;
  if (selector.className) {
    const classes = (node.attrs.class ?? "").split(/\s+/);
    if (!classes.includes(selector.className)) return false;
  }
  for (const cond of selector.attrConditions) {
    const actual = node.attrs[cond.name.toLowerCase()];
    if (actual === undefined) return false;
    if (cond.op === "exists") continue;
    if (cond.op === "=" && actual !== cond.value) return false;
    if (cond.op === "^=" && !actual.startsWith(cond.value)) return false;
    if (cond.op === "*=" && !actual.includes(cond.value)) return false;
  }
  return true;
}

/** Depth-first walk collecting every descendant (not including `root` itself) matching
 * `selector` — `Mojo::DOM`'s own `->find` searches the full subtree, not just direct children. */
function findAll(root: RawNode, selector: ParsedSelector, caseSensitive: boolean): RawNode[] {
  const out: RawNode[] = [];
  const visit = (node: RawNode) => {
    for (const child of node.children) {
      if (nodeMatches(child, selector, caseSensitive)) out.push(child);
      visit(child);
    }
  };
  visit(root);
  return out;
}

/** This element's own direct text content — concatenation of its immediate text-node children
 * only, matching `Mojo::DOM`'s own `->text` (descendant elements' text is excluded; use `->find`
 * + map over `->text` to collect nested text, same as real `Mojo::DOM` requires). Entity-decoded
 * via the same `htmlUnescape` used elsewhere, since real markup routinely carries `&amp;`-escaped
 * text nodes. */
function nodeText(node: RawNode): string {
  return globalThis.legacyCompat.htmlUnescape(node.ownText).trim();
}

/** Builds a `LegacyHttpResult` whose `dom` getter lazily parses `body` on first access (see that
 * interface's own docs for why this is a getter rather than parsing eagerly on every response). */
function makeHttpResult(body: string, code: number, headers: Headers): LegacyHttpResult {
  return {
    body,
    code,
    get dom() {
      return wrapNode(parseMarkup(body, false), false, body);
    },
    get json() {
      try {
        return JSON.parse(body);
      } catch {
        return undefined;
      }
    },
    // `Mojo::Message::is_error` — true for any 4xx/5xx status (real `Mojo::Message::Response`
    // reserves this for the response-side subclass; every legacy `->result->is_error` call site
    // in the corpus only ever checks it on a response, never a request, so it's fine to expose it
    // unconditionally here rather than modeling the request/response class split at all).
    get is_error() {
      return code >= 400;
    },
    // `Mojo::Headers`'s own `->location` accessor, the only one anything in the legacy corpus
    // reads off a response's headers — not a general header-access surface (`Mojo::Headers` has
    // many more; this is scoped to exactly what's actually used).
    get headers() {
      return { location: headers.get("Location") ?? undefined };
    },
  };
}

/** `caseSensitive` threads through every node wrapped from the same parse tree (a node found via
 * `->find` on an XML-parsed tree must keep comparing its *own* further `->at`/`->find` selectors
 * case-sensitively too — there's no per-node signal for this otherwise, since `RawNode` itself
 * doesn't record which mode produced it). `sourceMarkup` is the original string this whole tree
 * was parsed from, threaded the same way so `->to_string` works from any node reached via
 * `->parent`/`->at`/`->find` navigation, not just the root. */
function wrapNode(node: RawNode, caseSensitive: boolean, sourceMarkup: string): LegacyDomNode {
  const wrapped: LegacyDomNode = {
    get text() {
      return nodeText(node);
    },
    attr(name: string) {
      return node.attrs[name.toLowerCase()];
    },
    get parent() {
      return node.parent ? wrapNode(node.parent, caseSensitive, sourceMarkup) : undefined;
    },
    at(selector: string) {
      const [first] = findAll(node, parseSelector(selector), caseSensitive);
      return first ? wrapNode(first, caseSensitive, sourceMarkup) : undefined;
    },
    find(selector: string) {
      const matches = findAll(node, parseSelector(selector), caseSensitive).map((n) =>
        wrapNode(n, caseSensitive, sourceMarkup)
      );
      return Object.assign(matches, {
        each<T>(fn: (n: LegacyDomNode, i: number) => T): T[] {
          return matches.map((n, i) => fn(n, i));
        },
      });
    },
    children(selector?: string) {
      const parsed = selector ? parseSelector(selector) : undefined;
      const matches = node.children
        .filter((child) => !parsed || nodeMatches(child, parsed, caseSensitive))
        .map((child) => wrapNode(child, caseSensitive, sourceMarkup));
      return Object.assign(matches, {
        each<T>(fn: (n: LegacyDomNode, i: number) => T): T[] {
          return matches.map((n, i) => fn(n, i));
        },
      });
    },
    toString() {
      return sourceMarkup;
    },
  };
  return wrapped;
}

globalThis.legacyCompat = {
  reverse<T>(list: readonly T[]): T[] {
    return [...list].reverse();
  },
  chomp(s: string): string {
    return s.endsWith("\n") ? s.slice(0, -1) : s;
  },
  sprintf(format: string, ...args: unknown[]): string {
    let i = 0;
    return format.replace(
      /%(-?\d+)?(?:\.(\d+))?([sdf%])/g,
      (_match, width: string | undefined, precision: string | undefined, conv: string) => {
        if (conv === "%") return "%";
        const arg = args[i++];
        let str: string;
        if (conv === "d") str = String(Math.trunc(Number(arg)));
        else if (conv === "f") str = Number(arg).toFixed(precision ? Number(precision) : 6);
        else str = String(arg);
        if (width) {
          const w = Number(width);
          str = w < 0 ? str.padEnd(-w) : str.padStart(w);
        }
        return str;
      },
    );
  },
  userAgent(): LegacyUserAgent {
    const cookies: LegacyCookie[] = [];
    let followRedirects = false;
    // Set via `transactor.name(...)` (always just `User-Agent`) and/or `on('start', ...)` (any
    // header the handler's synthetic `tx.req.headers.header(...)` calls choose) — see
    // `LegacyUserAgent.on`'s docs above for why a one-shot registration-time call stands in for
    // real per-request firing here.
    const defaultHeaders: Record<string, string> = {};

    // Perl's own `Mojo::UserAgent` matches a cookie's `domain` against the request host by exact
    // match or parent-domain suffix (`.example.org` cookies also apply to `sub.example.org`).
    const domainMatches = (host: string, cookieDomain: string): boolean =>
      host === cookieDomain || host.endsWith(`.${cookieDomain}`);

    // ...and matches `path` by prefix: `/` (or no path at all) applies everywhere, `/foo` applies
    // to `/foo` itself and anything under `/foo/...`, but *not* to an unrelated `/foobar`.
    const pathMatches = (requestPath: string, cookiePath: string): boolean => {
      if (!cookiePath || cookiePath === "/") return true;
      if (requestPath === cookiePath) return true;
      const prefix = cookiePath.endsWith("/") ? cookiePath : `${cookiePath}/`;
      return requestPath.startsWith(prefix);
    };

    // Stored per `(name, domain, path)`, matching real cookie-jar semantics: setting a cookie
    // already in the jar for that exact scope replaces its value instead of appending a stale
    // duplicate the jar would otherwise keep sending forever.
    const store = (cookie: LegacyCookie) => {
      const idx = cookies.findIndex(
        (c) => c.name === cookie.name && c.domain === cookie.domain && c.path === cookie.path,
      );
      if (idx >= 0) cookies[idx] = cookie;
      else cookies.push(cookie);
    };

    const cookieHeaderFor = (url: string): string => {
      const { hostname, pathname } = new URL(url);
      return cookies
        .filter((c) => domainMatches(hostname, c.domain) && pathMatches(pathname, c.path))
        .map((c) => `${c.name}=${c.value}`)
        .join("; ");
    };

    // Real `Mojo::UserAgent`/browser cookie jars don't just send back whatever was explicitly
    // `->add()`-ed — they also capture whatever the *server* sets via `Set-Cookie` response
    // headers, so a later request in the same session picks up e.g. a freshly-issued session
    // cookie without the plugin having to parse and re-add it by hand.
    const captureSetCookies = (url: string, res: Response) => {
      for (const raw of res.headers.getSetCookie()) {
        const [nameValue, ...attrs] = raw.split(";").map((p) => p.trim());
        const eq = nameValue.indexOf("=");
        if (eq < 0) continue;
        const name = nameValue.slice(0, eq).trim();
        const value = nameValue.slice(eq + 1).trim();
        let domain = new URL(url).hostname;
        let path = "/";
        for (const attr of attrs) {
          const attrEq = attr.indexOf("=");
          if (attrEq < 0) continue;
          const key = attr.slice(0, attrEq).trim().toLowerCase();
          const val = attr.slice(attrEq + 1).trim();
          if (key === "domain" && val) domain = val.replace(/^\./, "");
          else if (key === "path" && val) path = val;
        }
        store({ name, value, domain, path });
      }
    };

    const ua: LegacyUserAgent = {
      cookie_jar: {
        add(cookie: LegacyCookie) {
          store(cookie);
        },
      },
      cookies,
      max_redirects(n: number): LegacyUserAgent {
        followRedirects = n > 0;
        return ua;
      },
      transactor: {
        name(value: string) {
          defaultHeaders["User-Agent"] = value;
        },
      },
      on(event, handler) {
        if (event !== "start") return;
        handler(ua, {
          req: {
            headers: {
              header(name: string, value: string) {
                defaultHeaders[name] = value;
              },
            },
          },
        });
      },
      async get(url: string, headers?: Record<string, string>) {
        const res = await fetch(url, {
          headers: { ...defaultHeaders, ...headers, Cookie: cookieHeaderFor(url) },
          redirect: followRedirects ? "follow" : "manual",
        });
        captureSetCookies(url, res);
        return { result: makeHttpResult(await res.text(), res.status, res.headers) };
      },
      async post(url: string, kind: "form" | "json", data) {
        const isJson = kind === "json";
        const res = await fetch(url, {
          method: "POST",
          headers: {
            ...defaultHeaders,
            Cookie: cookieHeaderFor(url),
            "Content-Type": isJson ? "application/json" : "application/x-www-form-urlencoded",
          },
          body: isJson
            ? JSON.stringify(data)
            : new URLSearchParams(data as Record<string, string>).toString(),
          redirect: followRedirects ? "follow" : "manual",
        });
        captureSetCookies(url, res);
        return { result: makeHttpResult(await res.text(), res.status, res.headers) };
      },
      headers: defaultHeaders,
    };
    return ua;
  },
  getLogger(name: string, category: string): LegacyLogger {
    const line = (level: string, msg: string) =>
      console.error(`[${category}] [${level}] ${name}: ${msg}`);
    return {
      debug: (msg) => line("debug", msg),
      info: (msg) => line("info", msg),
      warn: (msg) => line("warn", msg),
      error: (msg) => line("error", msg),
    };
  },
  htmlUnescape(s: string): string {
    // The HTML5 named entities actually seen across the legacy plugin corpus's scraped
    // titles/summaries — not the full ~2000-entry HTML5 named character reference table, which
    // would be pure dead weight for what this corpus exercises in practice.
    const named: Record<string, string> = {
      amp: "&",
      lt: "<",
      gt: ">",
      quot: '"',
      apos: "'",
      nbsp: " ",
      mdash: "—",
      ndash: "–",
      hellip: "…",
      rsquo: "’",
      lsquo: "‘",
      rdquo: "”",
      ldquo: "“",
    };
    return s.replace(/&(#x?[0-9a-fA-F]+|[a-zA-Z]+);/g, (full, entity: string) => {
      if (entity[0] === "#") {
        const codePoint = entity[1] === "x" || entity[1] === "X"
          ? parseInt(entity.slice(2), 16)
          : parseInt(entity.slice(1), 10);
        return Number.isNaN(codePoint) ? full : String.fromCodePoint(codePoint);
      }
      return named[entity] ?? full;
    });
  },
  parseHtml(markup: string, xml = false): LegacyDomNode {
    return wrapNode(parseMarkup(markup, xml), xml, markup);
  },
  sleep(seconds: number): Promise<void> {
    return new Promise((resolve) => setTimeout(resolve, seconds * 1000));
  },
  getVersion(): { version: string; homepage: string } {
    return { version: "0.1.0", homepage: "https://github.com/thelastfantasy/LANrurugi" };
  },
  refType(x: unknown): string {
    if (Array.isArray(x)) return "ARRAY";
    if (x !== null && typeof x === "object") return "HASH";
    return "";
  },
  trim(s: string | null | undefined): string {
    return s == null ? "" : s.trim();
  },
  fileparse(path: string): [string, string, string] {
    const lastSlash = path.lastIndexOf("/");
    const dir = lastSlash >= 0 ? path.slice(0, lastSlash + 1) : "./";
    const base = lastSlash >= 0 ? path.slice(lastSlash + 1) : path;
    const dotIdx = base.lastIndexOf(".");
    if (dotIdx > 0) {
      return [base.slice(0, dotIdx), dir, base.slice(dotIdx)];
    }
    return [base, dir, ""];
  },
  redis_decode(s: string): string {
    return s;
  },
};

// Re-exposed as a global so every plugin file can `throw new PluginErrorException(...)` without
// its own `import` (see this file's own top-of-file comment on the import right above for why a
// plugin file can't statically `import` `plugin-sdk.ts` at all).
globalThis.PluginErrorException = PluginErrorException;

const [pluginsDir, namespace] = Deno.args;
if (!pluginsDir || !namespace) {
  console.error("usage: dispatcher.ts <plugins_dir> <namespace>");
  Deno.exit(1);
}

const modulePromise = import(`file://${pluginsDir}/${namespace}.ts`);

const encoder = new TextEncoder();

function writeLine(obj: unknown) {
  Deno.stdout.writeSync(encoder.encode(JSON.stringify(obj) + "\n"));
}

async function handleRequest(req: PluginRequest) {
  try {
    const mod = await modulePromise;
    let result: unknown;
    switch (req.method) {
      case "plugin_info":
        result = await mod.pluginInfo();
        break;
      case "plugin_options":
        // Unlike `pluginInfo`, `pluginOptions` is optional (spec FR-015) — most plugins declare
        // no configurable settings at all, so its absence is normal, not an error.
        result = typeof mod.pluginOptions === "function" ? await mod.pluginOptions() : null;
        break;
      case "exec_metadata":
        result = await mod.execMetadata(req.args);
        break;
      case "exec_login":
        result = await mod.execLogin(req.args);
        break;
      case "exec_download":
        result = await mod.execDownload(req.args);
        break;
      case "exec_script":
        result = await mod.runScript(req.args);
        break;
      default:
        throw new Error(`unknown method: ${req.method}`);
    }
    writeLine({ request_id: req.request_id, ok: true, result });
  } catch (e) {
    const message = e instanceof Error ? e.message : String(e);
    // A plugin's own `PluginErrorException` (`plugin-sdk.ts`) carries a structured, translatable
    // `{error_code, data}` pair the Rust host can turn into `QueueError::PluginReported` instead
    // of an opaque string. Real `instanceof` works here (unlike before issue #86's SDK reorg) since
    // this file and every plugin file both `import` the identical `plugin-sdk.ts` module — Deno
    // caches modules by resolved URL, so both sides hold the same class reference. The
    // property-shape fallback below still covers a plugin that throws a plain `Error` with
    // `error_code`/`data` bolted on by hand instead of using the real class (never happens in this
    // repo's own corpus, but costs nothing to keep tolerating).
    const withCode = e as Error & { error_code?: unknown; data?: unknown };
    const error_code =
      e instanceof PluginErrorException
        ? e.error_code
        : e instanceof Error && typeof withCode.error_code === "string"
          ? withCode.error_code
          : undefined;
    const data =
      e instanceof PluginErrorException
        ? e.data
        : e instanceof Error && typeof withCode.data === "object"
          ? (withCode.data as Record<string, string | number> | undefined)
          : undefined;
    writeLine({
      request_id: req.request_id,
      ok: false,
      error: { message, kind: "plugin_error", error_code, data },
    });
  }
}

const decoder = new TextDecoder();
let buffer = "";
const reader = Deno.stdin.readable.getReader();

while (true) {
  const { value, done } = await reader.read();
  if (done) break;
  buffer += decoder.decode(value, { stream: true });

  let newlineIndex: number;
  while ((newlineIndex = buffer.indexOf("\n")) >= 0) {
    const line = buffer.slice(0, newlineIndex);
    buffer = buffer.slice(newlineIndex + 1);
    if (line.trim().length === 0) continue;
    const request = JSON.parse(line) as PluginRequest;
    // Intentionally not awaited: lets requests run concurrently.
    handleRequest(request);
  }
}
