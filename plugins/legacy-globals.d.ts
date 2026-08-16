// Ambient (host-provided) globals every plugin file under `plugins/` can call without any
// import/reference of its own — `legacyCompat.*` and the interfaces it returns
// (`LegacyUserAgent`, `LegacyDomNode`, ...), plus `PluginErrorException` (see its own docs below
// for why that one's here too, not just in `plugin-sdk.ts`). The real runtime implementations
// (`globalThis.legacyCompat = {...}`/`globalThis.PluginErrorException = ...`) live in
// `crates/lanrurugi-plugin/dispatcher/dispatcher.ts`, which references this file's types via its
// own `/// <reference types="../../plugins/legacy-globals.d.ts" />` directive (it's the host
// process, not a plugin file, so it's outside `plugins/tsconfig.json`'s `include` and still needs
// that one line).
//
// This file is a deliberate *global script*, not a module — no top-level `import`/`export`
// anywhere in it. That's what lets every `.ts` file under `plugins/` (via `plugins/tsconfig.json`
// establishing this directory as one TypeScript project) see these declarations with zero
// boilerplate of its own: TypeScript's own project-wide ambient-type behavior only applies to
// global scripts, not modules (a real, confirmed-live distinction — wrapping these same
// declarations in `declare global { ... }` here, which only makes sense *inside* a module, is
// rejected outright with `TS2669: Augmentations for the global scope can only be directly nested
// in external modules or ambient module declarations`). For the same reason, none of these
// declarations carry `export` — this file's job is purely to exist as ambient global state, not
// to be imported by name.
//
// `deno check` doesn't do this kind of ambient project-wide auto-discovery on its own (a real,
// separately-confirmed gotcha — see `plugins/deno.json`'s own docs on why `--config` must be
// passed explicitly there), so this file also gets referenced by `compilerOptions.types` in
// `plugins/deno.json` for that path. The two mechanisms are independent: this file makes
// **plain TypeScript tooling** (`tsc`, VS Code's built-in language service) see these globals with
// zero configuration on the *consuming* plugin file's part; `plugins/deno.json` + `--config` makes
// **Deno's own type checker** see the same thing. Both point at the same source of truth here so
// neither can drift from the other.
//
// Named `Legacy*` (not `Perl*`) because these interfaces don't model "the Perl language" in any
// general sense — they model this one legacy LANraragi codebase's own specific usage of two CPAN
// libraries (`Mojo::UserAgent`/`Mojo::DOM`); a `Perl*` prefix implied a language-level feature that
// isn't what's actually being represented here.

/** A cookie exactly as `Mojo::Cookie::Response->new(name => ..., value => ..., domain => ...,
 * path => ...)` shapes it in the legacy Perl source this was converted from —
 * `lanrurugi-plugin-converter` renders that legacy constructor call straight to an object literal
 * of this shape (see `render.rs::try_match_legacy_http_constructor`), so `cookie_jar` below takes
 * it as plain data rather than a real class instance. */
interface LegacyCookie {
  name: string;
  value: string;
  domain: string;
  path: string;
}

interface LegacyHttpResult {
  body: string;
  code: number;
  /** `$res->dom` (`Mojo::Message::Body`'s own DOM-parsing shortcut) — parses `body` as HTML
   * on first access. A getter (not parsed eagerly at fetch time) since most call sites only ever
   * read `.body` as plain text and never touch `.dom` at all. */
  readonly dom: LegacyDomNode;
  /** `$res->json` (`Mojo::Message::Body`'s own JSON-decoding shortcut) — parses `body` as JSON on
   * first access. Real `Mojo::Message::Body::json` returns `undef` on a parse failure rather than
   * throwing (legacy code routinely checks `if (defined $res->json)` or wraps the whole call in
   * `eval {}` expecting exactly that); matched here by returning `undefined` instead of letting
   * `JSON.parse` throw. */
  readonly json: unknown;
  /** `$res->is_error` (`Mojo::Message`'s own status-class check) — true for any 4xx/5xx HTTP
   * status. */
  readonly is_error: boolean;
  /** `$res->headers` — not a general `Mojo::Headers` shim, just the one accessor
   * (`->location`, read when manually following a 301/302 redirect with `max_redirects(0)`) any
   * real call site in the legacy plugin corpus actually reads off a response's headers. */
  readonly headers: { location: string | undefined };
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
interface LegacyDomNode {
  text: string;
  attr(name: string): string | undefined;
  parent: LegacyDomNode | undefined;
  at(selector: string): LegacyDomNode | undefined;
  find(selector: string): LegacyDomNode[] & { each<T>(fn: (node: LegacyDomNode, index: number) => T): T[] };
  /** `->children(selector?)` (`Mojo::DOM::children`) — direct children only (optionally filtered
   * by `selector`), distinct from `find`, which also matches nested descendants. */
  children(selector?: string): LegacyDomNode[] & { each<T>(fn: (node: LegacyDomNode, index: number) => T): T[] };
  /** `->to_string` — real `Mojo::DOM` re-serializes the (possibly-modified) tree back to markup;
   * this shim is read-only (no `->remove`/mutation support), so the original source markup never
   * actually changes after parsing, and returning that original string verbatim is equivalent to
   * a real serialization for every real call site in the legacy plugin corpus (both existing uses
   * — `EHentai.pm`/`Pixiv.pm` — only ever call it on the *root* `Mojo::DOM->new(...)`/`->dom`
   * result to substring-search the whole page, never on a sub-node after `->at()`/`->find()`). */
  toString(): string;
}

/** `$tx->req->headers->header(name => value)` — the one piece of `Mojo::Transaction` surface an
 * `->on('start', sub ($ua, $tx) {...})` handler (see `LegacyUserAgent.on` below) actually reaches
 * into across the legacy plugin corpus (always to set a static request header, never to read
 * anything back off `$tx`), so this is intentionally not a general `Mojo::Transaction` shim. */
interface LegacyTransaction {
  req: { headers: { header(name: string, value: string): void } };
}

/** `legacyCompat.userAgent()`'s return shape — enough of `Mojo::UserAgent`'s own chainable
 * surface (`->cookie_jar->add(...)`, `->max_redirects(n)->get(url)->result->body`) to cover what
 * the legacy plugin corpus actually calls, backed by `fetch()` instead of a real CPAN HTTP
 * client. Not a general Mojo::UserAgent reimplementation — no proxy/auth/multipart support, no
 * `Mojo::Message`-family object model, just the one GET/POST-with-cookies-and-redirects idiom
 * every login/download plugin so far has used. */
interface LegacyUserAgent {
  cookie_jar: { add(cookie: LegacyCookie): void };
  max_redirects(n: number): LegacyUserAgent;
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
  on(event: "start", handler: (ua: LegacyUserAgent, tx: LegacyTransaction) => void): void;
  /** `headers`, when given, are merged on top of this instance's own default headers (set via
   * {@linkcode transactor}/{@linkcode on}) for this one request only — mirrors real
   * `Mojo::UserAgent`'s own `->get($url => {'Header-Name' => 'value'})` calling convention. */
  get(url: string, headers?: Record<string, string>): Promise<{ result: LegacyHttpResult }>;
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
  ): Promise<{ result: LegacyHttpResult }>;
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
  cookies: LegacyCookie[];
}

/** `get_logger($name, $category)`/`get_plugin_logger()`'s return shape. The legacy versions write
 * to a real rotating log file on disk (`LANraragi::Utils::Logging`); most converted plugins don't
 * get filesystem write access under `declared_permissions` (constitution Principle IV), so this
 * logs to stderr instead — safe because `dispatcher.ts`'s own stdout is reserved for its
 * newline-delimited JSON protocol (see `writeLine` below) and stderr doesn't share that stream. */
interface LegacyLogger {
  debug(msg: string): void;
  info(msg: string): void;
  warn(msg: string): void;
  error(msg: string): void;
}

// deno-lint-ignore no-var
declare var legacyCompat: {
  /** Perl's `reverse(@list)` returns a *new* list; JS's `Array.prototype.reverse` mutates its
   * receiver in place. Spreads into a fresh array first so converted code doesn't pick up a
   * mutation side effect the original Perl never had. */
  reverse<T>(list: readonly T[]): T[];
  /** Perl's `chomp($x)` mutates `$x` in place, stripping exactly one trailing `"\n"` (Perl's
   * default `$/` record separator — not `"\r\n"`). JS can't mutate a caller's local binding
   * through a function call, so converted code must reassign the result itself:
   * `x = legacyCompat.chomp(x)`, mirroring `chomp $x;`'s own mutating spirit as closely as a
   * pure function can. */
  chomp(s: string): string;
  /** A minimal `sprintf` covering the format-spec subset actually used across the legacy
   * plugin corpus (`%s`, `%d`, `%f` with optional width/precision) — not a full reimplementation
   * of Perl's considerably larger `sprintf` grammar. */
  sprintf(format: string, ...args: unknown[]): string;
  /** `Mojo::UserAgent->new` — see `LegacyUserAgent`'s own docs for exactly what's covered. Every
   * plugin using this must still declare its own real target host(s) in `declared_permissions`
   * (constitution Principle IV); this shim runs inside that same already-permission-scoped
   * process, so it's bound by whatever `--allow-net` grant the plugin itself was started with,
   * same as a plugin calling `fetch()` directly would be. */
  userAgent(): LegacyUserAgent;
  /** `get_logger($name, $category)` — see `LegacyLogger`'s own docs for exactly what's covered. */
  getLogger(name: string, category: string): LegacyLogger;
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
   * `LegacyDomNode`'s own docs for what's covered. */
  parseHtml(markup: string, xml?: boolean): LegacyDomNode;
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

/** A plugin-authored, structured error — throw this (instead of a plain `throw new Error(...)`)
 * to report `{error_code, data}` through an exception rather than a `return {error}`. `error_code`
 * doubles as an i18n lookup key (see `crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`'s
 * `PluginError` for the full field-meaning docs — this class's fields mirror that interface
 * exactly). The dispatcher's own `catch` (`dispatcher.ts`'s `handleRequest`) uses real
 * `instanceof` against this exact class to extract `error_code`/`data` for the Rust host.
 *
 * The *real* class lives in `plugin-sdk.ts` and is exposed here only as a global
 * (`globalThis.PluginErrorException = PluginErrorException` in `dispatcher.ts`) so every plugin
 * file can use it with zero `import` of its own — a plugin file's static `import` specifier can't
 * reach `plugin-sdk.ts` at all, since `plugins_dir` and `plugin-sdk.ts`'s own directory
 * (`temp_dir`) have no fixed relative path between them (both are independent runtime CLI args).
 * `declare class` here (rather than `declare var ...: PluginError`) is what lets a plugin file
 * both `throw new PluginErrorException(...)` (construct) and `catch` it with `instanceof`
 * (matches the *value* `dispatcher.ts` actually assigns, a real ES class, not just its instance
 * shape). */
// deno-lint-ignore no-var
declare var PluginErrorException: {
  new (error_code: string, data?: Record<string, string | number>): Error & {
    error_code: string;
    data?: Record<string, string | number>;
  };
};

// ── hostArgs protocol types (issue #86) ───────────────────────────────────────────────────────
// `MetadataHostArgs`/`LoginHostArgs`/`ScriptHostArgs`/`DownloadHostArgs` moved here from
// `plugin-sdk.ts` (that file's own module doc comment explains why: `lanrurugi-plugin-converter`'s
// generated code references them by name via `extends Pick<..., "key1" | "key2">` in each entry
// point's own narrowed local interface, and a plugin file has no way to `import` an `export`ed
// symbol from `plugin-sdk.ts` — same unresolvable-relative-path problem as `PluginErrorException`
// above). `plugin-sdk.ts` remains the place to read these types' own full field-by-field docs in
// context alongside `PluginInfoResult`/the four `*Result` return shapes they pair with — only the
// two files' `deno doc`/`deno check` visibility differs, not which one is "more real."

/** `hostArgs` for `execMetadata`. In practice a converted plugin's generated entry point binds
 * the *whole* object to one variable (`let lrr_info = hostArgs as Record<string, any>;`) and reads
 * individual fields off it by name — this interface exists so a hand-written plugin (or
 * `plugin-sdk.ts`'s own docs) has something concrete to check field names against. */
interface MetadataHostArgs {
  /** Present when the call was made against a real archive (`GET /plugins/use_plugin?id=...`);
   * absent for a settings-page "test this plugin" dry run. */
  archive_id?: string;
  /** The archive's current stored `tags` string (comma-separated, possibly empty) — matches
   * legacy's own `exec_metadata_plugin` (`Model/Plugins.pm`), whose `%infohash` always includes
   * `existing_tags`. Several real converted plugins (`chaika`/`ehentai`/`fakku`/`hitomi`/`mems`/
   * `nhentai`) parse this for an existing `source:`-style tag as a fallback when no `arg`/oneshot
   * URL was given (e.g. every auto-run/watcher-triggered call, which never has one) — present
   * (possibly `""`) whenever `archive_id` is, absent only for a settings-page dry run with no real
   * archive at all. */
  existing_tags?: string;
  /** The archive's current stored `title` — same legacy `%infohash` field (`archive_title`).
   * Several real converted plugins (`chaika`/`ehentai`/`fakku`/`mems`/`pixiv`) fall back to parsing
   * an embedded ID out of this when neither `arg` nor `existing_tags` yielded one. Same presence
   * rule as {@linkcode existing_tags}. */
  archive_title?: string;
  /** A metadata plugin's cached thumbnail-image content hash, used by a handful of real converted
   * plugins (e.g. `chaika.ts`'s "look up by thumbnail" fallback path) to search a source site by
   * image rather than by title/ID. Currently always `""` in practice (`lanrurugi-api::plugins`
   * has no real thumbnail-hashing step wired up yet) — present in the wire contract so those
   * plugins' own reads still type-check and don't crash on `undefined`, not because a real value
   * is ever supplied today. */
  thumbnail_hash?: string;
  /** The single free-text "run once" value (see `PluginInfoResult.oneshot_arg` in `plugin-sdk.ts`)
   * — `undefined`/absent for any call that isn't a manual one-shot run. Distinct from {@linkcode
   * customargs}. */
  arg?: string;
  /** Exact duplicate of {@linkcode arg}, sent under legacy's own field name
   * (`$lrr_info->{oneshot_param}` — every legacy Perl plugin source reads it under this name, not
   * `arg`) alongside it so a converted plugin's untouched, unmodified-since-conversion field
   * access still works verbatim. A hand-written (non-converted) plugin should prefer {@linkcode
   * arg} — this field exists purely for source fidelity with the legacy corpus, not because the
   * two ever carry different values. */
  oneshot_param?: string;
  /** This plugin's own persisted per-parameter values, positionally matching `PluginInfoResult.
   * parameters` in `plugin-sdk.ts` (`customargs[0]` is `parameters[0]`'s saved value, etc.) —
   * always present, one entry per declared parameter, `""` for any never configured. */
  customargs: string[];
  /** The archive's on-disk path — fetched by the host once per call
   * (`lanrurugi-api::plugins::use_plugin_sync`) so a filename-deriving plugin (e.g. "Filename
   * Parsing") doesn't need its own round trip back into the API just to resolve an ID to a path. */
  file_path?: string;
  /** `file_path`'s own last-modified time (Unix seconds) — resolved host-side (a plain `stat`
   * call) alongside `file_path` itself so a plugin needing this (currently only
   * `plugins/metadata/dateadded.ts`) doesn't need its own filesystem read permission just for
   * this one call. Absent when `file_path` itself is absent, or the `stat` call failed. */
  file_modified_time?: number;
  /** A small, deliberately narrow subset of the server's global settings (`LRR_CONFIG`) a
   * metadata plugin might need to consult — currently just `usedateadded`/`usedatemodified`
   * (`plugins/metadata/dateadded.ts`'s own settings-driven defaults). Not the *entire* settings
   * hash (constitution Principle IV: no broader access than a plugin actually needs). */
  settings?: { usedateadded: boolean; usedatemodified: boolean };
  /** Another archive's stored `tags` string, resolved host-side by extracting a 40-character
   * archive ID out of this call's own {@linkcode arg} — currently only computed (and only
   * non-null) for `plugins/metadata/copyarchivetags.ts`, which has no other way to read a
   * *different* archive's metadata from inside the Deno sandbox (no direct storage access).
   * `null` when `arg` contains no valid ID, or that ID doesn't resolve to a real archive. */
  other_archive_tags?: string | null;
  /** Content of every file listed in this plugin's own `PluginInfoResult.sidecar_files`
   * (`plugin-sdk.ts`), keyed by that same filename — present (possibly empty) only when
   * `sidecar_files` was non-empty; a missing/unreadable/non-UTF-8 file is just absent from this
   * map rather than failing the whole call. */
  sidecar_files?: Record<string, string>;
  /** Populated only when this plugin (or the one it declares via `PluginInfoResult.login_from`)
   * needs a logged-in session — see `plugin-sdk.ts`'s `LoginResult` for where this comes from. A
   * converted plugin's entry point rehydrates this into a real `legacyCompat.userAgent()` instance
   * before its body runs (`render.rs`'s `render_user_agent_hydration_preamble`). */
  user_agent_cookies?: LegacyCookie[];
}

/** `hostArgs` for `execLogin` — the plugin's own persisted parameter values (credentials,
 * typically — see {@linkcode MetadataHostArgs.customargs}'s own docs, same shape), never an
 * `archive_id`/`file_path` (a login plugin never runs against a specific archive). Re-sent on
 * *every* call to whatever declares this plugin as its `login_from`, never a cached session, so
 * credentials always reflect the current saved settings. */
interface LoginHostArgs {
  customargs: string[];
}

/** One archive as passed into a `"script"`-type plugin that needs to inspect/rewrite tags across
 * the whole library (`ScriptHostArgs.archives`) — deliberately just the two fields any such script
 * plugin in this corpus actually needs, not the full archive record (constitution Principle IV). */
interface ScriptArchiveTags {
  id: string;
  tags: string;
}

/** `hostArgs` for `runScript` — legacy's three Scripts plugins operate library-wide (not against
 * one archive), so unlike {@linkcode MetadataHostArgs} there's no `archive_id`/`file_path` here
 * either. A Deno-sandboxed plugin has no direct Redis/archive-storage access, so any script that
 * needs to *read* every archive's tags (`nhsrcconv`) gets them pre-fetched into {@linkcode
 * archives} rather than reaching for storage itself; a script that needs to *write* tags returns
 * its intended changes in `ScriptResult.updates` (`plugin-sdk.ts`) instead of writing them
 * directly — the host applies them after the call returns (same shape both ways, so a plugin's
 * own transform logic is the only real content). `urlfinder`'s one host-computed lookup
 * (`Model::Stats::is_url_recorded`, a `LRR_URLMAP` hash read this repo doesn't maintain — resolved
 * host-side as a full tag-scan instead, mirroring `GET /database/stats`'s own same simplification)
 * is exposed as {@linkcode existing_archive_id}. */
interface ScriptHostArgs {
  /** The single free-text "run once" value — same meaning as {@linkcode MetadataHostArgs.arg},
   * named to match legacy's own `$lrr_info->{oneshot_param}` field exactly (`SourceFinder.pm`'s
   * only real input). */
  oneshot_param?: string;
  /** This plugin's own persisted per-parameter values — same meaning as {@linkcode
   * MetadataHostArgs.customargs}. */
  customargs: string[];
  /** Every archive's `id`/`tags`, host-fetched once before the call — present (possibly empty)
   * only for a script plugin whose namespace the host recognizes as needing it (currently just
   * `script/nhentaisourceconverter`); `undefined` for every other script plugin, including a
   * third-party one uploaded via `POST /plugins/upload` (no way to know it needs this without a
   * declarative opt-in this SDK doesn't have yet — such a plugin can still request a narrower,
   * per-archive equivalent through the same mechanism {@linkcode MetadataHostArgs.other_archive_tags}
   * uses, if a future need arises). */
  archives?: ScriptArchiveTags[];
  /** Host-resolved archive ID whose `source:` tag matches {@linkcode oneshot_param} (after
   * trimming and the E-Hentai/ExHentai domain-alias special case) — present (possibly `null`) only
   * for `script/urlfinder`, `undefined` for every other script plugin. */
  existing_archive_id?: string | null;
  /** The library's real archive-directory root, absolute path — present only for
   * `script/foldertocat`, which (unlike every other plugin in this corpus) needs genuine, broad
   * filesystem read access to walk the whole content tree itself via `Deno.readDir`
   * (`declared_permissions.read: true`, an unrestricted `--allow-read` grant — see
   * `lanrurugi_plugin::pool::Worker::spawn`), matching legacy's own `FolderToCat.pm::run_script`
   * (`File::Find` over `LANraragi::Model::Config->get_userdir`) exactly rather than having the
   * host pre-walk it and hand over a file list, which would make a since-requested Rust-vs-TS
   * performance comparison meaningless (the I/O would happen only once, on the Rust side, either
   * way). `undefined` for every other script plugin. */
  library_path?: string;
  /** Maps each already-catalogued archive's on-disk path to its ID — present only for
   * `script/foldertocat`, needed to resolve the files `Deno.readDir` finds back to real archive
   * IDs before returning its computed category groupings (a plugin has no direct storage access
   * to look this up itself). `undefined` for every other script plugin. */
  archive_id_by_path?: Record<string, string>;
}

/** `hostArgs` for `execDownload` — the user-supplied URL/ID to download, plus an optional target
 * category (`POST /download_url?url=...&catid=...`). Unlike `execMetadata` there's no
 * `archive_id`/`file_path`: nothing has been catalogued yet at this point, that's the whole job
 * this entry point exists to do. */
interface DownloadHostArgs {
  /** Mirrors legacy's own `%infohash` shape (`~/LANraragi/lib/LANraragi/Model/Plugins.pm:171-175`,
   * `url => $input`) — every real download plugin in this corpus reads `hostArgs.url`. */
  url: string;
  category?: string;
  user_agent_cookies?: LegacyCookie[];
  /** This plugin's own persisted per-parameter values — same meaning as {@linkcode
   * MetadataHostArgs.customargs}. Legacy passes the equivalent (`%settings`, converted from its
   * own persisted array into a keyed hash) as a *separate* positional argument to
   * `provide_url($lrr_info, %params)` (`~/LANraragi/lib/LANraragi/Model/Plugins.pm:163-179`)
   * rather than folding it into the info hash the way this repo does — e.g. `ehentai.ts`'s own
   * `forceresampled` toggle reads `hostArgs.customargs[0]` exactly like a metadata plugin would. */
  customargs: string[];
}
