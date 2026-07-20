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
// reasoning is why `perlCompat` below is a plain global assignment rather than a separate module
// a plugin would need to `import` — that would need its own path added to the plugin's
// `--allow-read` grant (see `lanrurugi_plugin::pool::Worker::spawn`'s default, which is scoped to
// exactly this dispatcher file and the one plugin file being run); a global needs no extra grant
// at all, since it's set in this same already-trusted process before the plugin module loads.

interface PluginRequest {
  request_id: string;
  plugin: string;
  method: string;
  args: unknown;
}

/** A cookie as `Mojo::Cookie::Response->new(name => ..., value => ..., domain => ..., path =>
 * ...)` shapes it — `lanrurugi-plugin-converter` renders that legacy constructor call straight to
 * an object literal of this shape (see `render.rs::try_match_legacy_http_constructor`), so `cookie_jar`
 * below takes it as plain data rather than a real class instance. */
interface PerlCookie {
  name: string;
  value: string;
  domain: string;
  path: string;
}

interface PerlHttpResult {
  body: string;
  code: number;
  /** `$res->dom` (`Mojo::Message::Body`'s own DOM-parsing shortcut) — parses `body` as HTML
   * on first access. A getter (not parsed eagerly at fetch time) since most call sites only ever
   * read `.body` as plain text and never touch `.dom` at all. */
  readonly dom: PerlDomNode;
  /** `$res->json` (`Mojo::Message::Body`'s own JSON-decoding shortcut) — parses `body` as JSON on
   * first access. Real `Mojo::Message::Body::json` returns `undef` on a parse failure rather than
   * throwing (legacy code routinely checks `if (defined $res->json)` or wraps the whole call in
   * `eval {}` expecting exactly that); matched here by returning `undefined` instead of letting
   * `JSON.parse` throw. */
  readonly json: unknown;
}

/** `Mojo::DOM`'s return shape for `->at(selector)`/`->find(selector)` results — enough of its
 * chainable node API (`->text`, `->attr(name)`, `->parent`, `->find(selector)->each`) to cover
 * every call site across the legacy plugin corpus (verified: every `->at(...)`/`->find(...)`
 * selector string across `~/LANraragi/lib/LANraragi/Plugin/**\/*.pm` is a single simple selector —
 * a tag name, `.class`, `#id`, or `[attr=val]`/`[attr^=val]`, never a descendant combinator or
 * pseudo-class). Deliberately not a real DOM (no `querySelector`, no live tree, no `Node`
 * interface) and not backed by a third-party HTML-parsing library (`dispatcher.ts`'s own docs:
 * no external module imports at all, so a plugin's `--allow-net`/`--allow-read` grant is never
 * implicitly widened by the dispatcher's own needs) — a small hand-rolled parser and selector
 * matcher, sized to exactly what this corpus exercises. */
interface PerlDomNode {
  text: string;
  attr(name: string): string | undefined;
  parent: PerlDomNode | undefined;
  at(selector: string): PerlDomNode | undefined;
  find(selector: string): PerlDomNode[] & { each<T>(fn: (node: PerlDomNode, index: number) => T): T[] };
  /** `->to_string` — real `Mojo::DOM` re-serializes the (possibly-modified) tree back to markup;
   * this shim is read-only (no `->remove`/mutation support), so the original source markup never
   * actually changes after parsing, and returning that original string verbatim is equivalent to
   * a real serialization for every real call site in the legacy plugin corpus (both existing uses
   * — `EHentai.pm`/`Pixiv.pm` — only ever call it on the *root* `Mojo::DOM->new(...)`/`->dom`
   * result to substring-search the whole page, never on a sub-node after `->at()`/`->find()`). */
  toString(): string;
}

/** `perlCompat.userAgent()`'s return shape — enough of `Mojo::UserAgent`'s own chainable
 * surface (`->cookie_jar->add(...)`, `->max_redirects(n)->get(url)->result->body`) to cover what
 * the legacy plugin corpus actually calls, backed by `fetch()` instead of a real CPAN HTTP
 * client. Not a general Mojo::UserAgent reimplementation — no proxy/auth/multipart support, no
 * `Mojo::Message`-family object model, just the one GET/POST-with-cookies-and-redirects idiom
 * every login/download plugin so far has used. */
/** `$tx->req->headers->header(name => value)` — the one piece of `Mojo::Transaction` surface an
 * `->on('start', sub ($ua, $tx) {...})` handler (see `PerlUserAgent.on` below) actually reaches
 * into across the legacy plugin corpus (always to set a static request header, never to read
 * anything back off `$tx`), so this is intentionally not a general `Mojo::Transaction` shim. */
interface PerlTransaction {
  req: { headers: { header(name: string, value: string): void } };
}

interface PerlUserAgent {
  cookie_jar: { add(cookie: PerlCookie): void };
  max_redirects(n: number): PerlUserAgent;
  /** `$ua->transactor->name($value)` — sets the default `User-Agent` header every subsequent
   * `get`/`post` call on this instance sends. */
  transactor: { name(value: string): void };
  /** `$ua->on(start => sub ($ua, $tx) {...})` — real `Mojo::UserAgent` fires this before *every*
   * request; every real call site in the legacy plugin corpus only uses it to set one or more
   * static headers via `$tx->req->headers->header(...)`, never anything request-specific (e.g.
   * inspecting `$tx->req->url`), so it's faithfully enough modeled by running `handler` once,
   * immediately, against a synthetic `tx` whose header setter writes into this instance's default
   * headers (applied to every `get`/`post` call from then on) rather than actually re-invoking it
   * per request. Only the `"start"` event is recognized — legacy's other events
   * (`error`/`finish`/etc.) have no real use in this corpus and aren't modeled. */
  on(event: "start", handler: (ua: PerlUserAgent, tx: PerlTransaction) => void): void;
  get(url: string): Promise<{ result: PerlHttpResult }>;
  /** `kind` mirrors Mojo::UserAgent's own `->post($url => form => {...})` /
   * `->post($url => json => {...})` calling convention — `form` URL-encodes `data` as
   * `application/x-www-form-urlencoded` (string values only, matching real HTML form semantics);
   * `json` sends it as a `application/json` body (`data` may be any JSON-serializable value —
   * verified against the one real `json`-kind call site in the corpus, which passes a nested
   * object with an array value, not just flat strings). */
  post(
    url: string,
    kind: "form" | "json",
    data: Record<string, string> | Record<string, unknown>,
  ): Promise<{ result: PerlHttpResult }>;
  /** A plain, JSON-serializable snapshot of this instance's cookie jar (the *same* live array the
   * `get`/`post`/`cookie_jar.add` closures above mutate, not a copy) — every other property here
   * is a function, which `JSON.stringify` silently drops, so a `do_login`-derived plugin's
   * `return ua;` would otherwise cross `handleRequest`'s `writeLine` (below) as an empty object.
   * The host (`lanrurugi-api::plugins::with_login_cookies`) reads this off `exec_login`'s result
   * and threads it back into a later `exec_metadata`/`exec_download`/`exec_script` call as
   * `hostArgs.user_agent_cookies`, which `lanrurugi-plugin-converter`'s generated entry points
   * rehydrate into a fresh `userAgent()` instance (see `render.rs`'s
   * `render_user_agent_hydration_preamble`) — a live object with closures can't itself cross that
   * same JSON boundary, so this plain-array snapshot is what actually gets to do the traveling. */
  cookies: PerlCookie[];
}

/** `get_logger($name, $category)`/`get_plugin_logger()`'s return shape. The legacy versions write
 * to a real rotating log file on disk (`LANraragi::Utils::Logging`); most converted plugins don't
 * get filesystem write access under `declared_permissions` (constitution Principle IV), so this
 * logs to stderr instead — safe because `dispatcher.ts`'s own stdout is reserved for its
 * newline-delimited JSON protocol (see `writeLine` below) and stderr doesn't share that stream. */
interface PerlLogger {
  debug(msg: string): void;
  info(msg: string): void;
  warn(msg: string): void;
  error(msg: string): void;
}

// Perl-semantics-preserving helpers for `lanrurugi-plugin-converter`'s output — centralized here
// (rather than inlined at every call site during conversion) specifically for the cases where a
// literal 1:1 substitution would be wrong or where Perl has no native JS equivalent at all, so a
// semantic fix only ever needs to happen in one place instead of in every already-converted
// plugin file. The `declare global` block right below exists only here at the *implementation*
// site for documentation; `lanrurugi-plugin-converter` repeats just the type signature (not the
// implementation) at the top of every file it generates, since Deno's per-file type-checking
// can't be relied on to see a `declare global` written in this unrelated dispatcher file.
declare global {
  // deno-lint-ignore no-var
  var perlCompat: {
    /** Perl's `reverse(@list)` returns a *new* list; JS's `Array.prototype.reverse` mutates its
     * receiver in place. Spreads into a fresh array first so converted code doesn't pick up a
     * mutation side effect the original Perl never had. */
    reverse<T>(list: readonly T[]): T[];
    /** Perl's `chomp($x)` mutates `$x` in place, stripping exactly one trailing `"\n"` (Perl's
     * default `$/` record separator — not `"\r\n"`). JS can't mutate a caller's local binding
     * through a function call, so converted code must reassign the result itself:
     * `x = perlCompat.chomp(x)`, mirroring `chomp $x;`'s own mutating spirit as closely as a
     * pure function can. */
    chomp(s: string): string;
    /** A minimal `sprintf` covering the format-spec subset actually used across the legacy
     * plugin corpus (`%s`, `%d`, `%f` with optional width/precision) — not a full reimplementation
     * of Perl's considerably larger `sprintf` grammar. */
    sprintf(format: string, ...args: unknown[]): string;
    /** `Mojo::UserAgent->new` — see `PerlUserAgent`'s own docs for exactly what's covered. Every
     * plugin using this must still declare its own real target host(s) in `declared_permissions`
     * (constitution Principle IV); this shim runs inside that same already-permission-scoped
     * process, so it's bound by whatever `--allow-net` grant the plugin itself was started with,
     * same as a plugin calling `fetch()` directly would be. */
    userAgent(): PerlUserAgent;
    /** `get_logger($name, $category)` — see `PerlLogger`'s own docs for exactly what's covered. */
    getLogger(name: string, category: string): PerlLogger;
    /** `Mojo::Util::html_unescape` — decodes HTML entities (named: `&amp;`/`&lt;`/`&quot;`/etc.,
     * and numeric: `&#39;`/`&#x27;`). No native JS equivalent outside a real DOM (a `textarea`
     * element's `.value` trick, unavailable in this Deno sandbox — there's no `document`), so
     * reimplemented directly against the HTML5 entity table's most common names rather than
     * pulling in a full HTML entity library for what the legacy plugin corpus only ever uses on
     * scraped titles/summaries (a handful of the same few entities in practice). */
    htmlUnescape(s: string): string;
    /** `Mojo::DOM->new(html)` / `Mojo::DOM->new->xml(1)->parse(xml)` — parses markup and returns
     * its root node. `xml` toggles case-sensitive tag matching (real XML/custom elements like
     * `<Genre>` in a ComicInfo.xml file) vs HTML's traditionally case-insensitive tag names; see
     * `PerlDomNode`'s own docs for what's covered. */
    parseHtml(markup: string, xml?: boolean): PerlDomNode;
    /** Perl's `sleep($seconds)` blocks synchronously; JS has no synchronous sleep, so this is a
     * `setTimeout`-backed `Promise` instead — every call site must `await` it (the converter's
     * own `sleep` mapping in `render.rs` always emits the `await`, forcing the enclosing function
     * `async` the same way an awaited `->get()`/`->post()` Mojo call already does). */
    sleep(seconds: number): Promise<void>;
    /** `LANraragi::Utils::Generic::get_version` — legacy reads this straight out of its own
     * `package.json` (`version`/`homepage` fields) once and caches it; this returns the
     * equivalent static values for this project instead of trying to read a Rust crate's
     * `Cargo.toml` from within the Deno sandbox. Update these two literals if
     * `Cargo.toml`'s `[workspace.package]` `version`/`repository` ever changes. */
    getVersion(): { version: string; homepage: string };
    /** Perl's `ref($x)` builtin — returns `"HASH"`/`"ARRAY"` for a hashref/arrayref, or `""` for
     * anything else (a plain scalar, `undef`, etc.), matching Perl's own string-truthiness rule
     * closely enough that `ref($x)` used as a bare boolean condition (`return if ref $x;`) works
     * unmodified once translated. Only hashref/arrayref detection — real Perl `ref()` also
     * recognizes `CODE`/`SCALAR`/blessed-object-class-name, none of which the legacy plugin
     * corpus's own use of `ref()` ever tests for. */
    refType(x: unknown): string;
    /** `LANraragi::Utils::String::trim` — strips leading/trailing whitespace, returning `""` for
     * `null`/`undefined` instead of throwing (unlike JS's own `String.prototype.trim`, which
     * requires a real string receiver). */
    trim(s: string | null | undefined): string;
    /** `File::Basename::fileparse($path, $suffix_pattern)` — every real call site in the legacy
     * plugin corpus passes the same "match a trailing dot plus any run of non-dot characters"
     * suffix pattern (i.e. "the last extension"), so the second argument is accepted but ignored
     * rather than genuinely interpreting an arbitrary suffix pattern. Returns
     * `[name-without-extension, directory-with-trailing-slash, extension-with-leading-dot]`,
     * matching Perl's own return order. */
    fileparse(path: string, suffixPattern?: unknown): [string, string, string];
    /** `LANraragi::Utils::Redis::redis_decode` — legacy re-decodes a value read back from Redis
     * as UTF-8, since Perl's own Redis client hands back raw bytes. Deno's Redis client (and
     * every string this dispatcher otherwise touches) is already a proper UTF-8 JS string by the
     * time a plugin sees it, so this is a pure pass-through. */
    redis_decode(s: string): string;
  };
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
  return globalThis.perlCompat.htmlUnescape(node.ownText).trim();
}

/** Builds a `PerlHttpResult` whose `dom` getter lazily parses `body` on first access (see that
 * interface's own docs for why this is a getter rather than parsing eagerly on every response). */
function makeHttpResult(body: string, code: number): PerlHttpResult {
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
  };
}

/** `caseSensitive` threads through every node wrapped from the same parse tree (a node found via
 * `->find` on an XML-parsed tree must keep comparing its *own* further `->at`/`->find` selectors
 * case-sensitively too — there's no per-node signal for this otherwise, since `RawNode` itself
 * doesn't record which mode produced it). `sourceMarkup` is the original string this whole tree
 * was parsed from, threaded the same way so `->to_string` works from any node reached via
 * `->parent`/`->at`/`->find` navigation, not just the root. */
function wrapNode(node: RawNode, caseSensitive: boolean, sourceMarkup: string): PerlDomNode {
  const wrapped: PerlDomNode = {
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
        each<T>(fn: (n: PerlDomNode, i: number) => T): T[] {
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

globalThis.perlCompat = {
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
  userAgent(): PerlUserAgent {
    const cookies: PerlCookie[] = [];
    let followRedirects = false;
    // Set via `transactor.name(...)` (always just `User-Agent`) and/or `on('start', ...)` (any
    // header the handler's synthetic `tx.req.headers.header(...)` calls choose) — see
    // `PerlUserAgent.on`'s docs above for why a one-shot registration-time call stands in for
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
    const store = (cookie: PerlCookie) => {
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

    const ua: PerlUserAgent = {
      cookie_jar: {
        add(cookie: PerlCookie) {
          store(cookie);
        },
      },
      cookies,
      max_redirects(n: number): PerlUserAgent {
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
      async get(url: string) {
        const res = await fetch(url, {
          headers: { ...defaultHeaders, Cookie: cookieHeaderFor(url) },
          redirect: followRedirects ? "follow" : "manual",
        });
        captureSetCookies(url, res);
        return { result: makeHttpResult(await res.text(), res.status) };
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
        return { result: makeHttpResult(await res.text(), res.status) };
      },
    };
    return ua;
  },
  getLogger(name: string, category: string): PerlLogger {
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
  parseHtml(markup: string, xml = false): PerlDomNode {
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
    writeLine({
      request_id: req.request_id,
      ok: false,
      error: { message, kind: "plugin_error" },
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
