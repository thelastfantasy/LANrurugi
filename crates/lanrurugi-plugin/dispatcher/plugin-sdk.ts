/// <reference types="../../../plugins/legacy-globals.d.ts" />
/**
 * # LANrurugi Plugin SDK
 *
 * A plugin is a single `.ts` file dropped under `plugins/<category>/` (or uploaded through
 * `POST /plugins/upload`, which lands in `plugins/custom/<category>/` instead — see
 * `lanrurugi-api::plugins::upload_plugin`). It runs inside its own Deno subprocess
 * (`crates/lanrurugi-plugin/src/pool.rs`), started with exactly the `--allow-net`/`--allow-read`
 * grant its own {@linkcode PluginInfoResult.declared_permissions} asked for — nothing broader
 * (constitution Principle IV). This module documents that contract as real, type-checked TS
 * (generate a browsable copy with `mise run plugin-sdk-docs`) rather than free-hand prose, so it
 * can't drift from what the host (`lanrurugi-api::plugins`) and the dispatcher
 * (`dispatcher.ts`, doc'd alongside this file) actually do.
 *
 * Every plugin file must export exactly two things:
 *
 * 1. `pluginInfo(): {@linkcode PluginInfoResult}` — synchronous, no side effects. Called by
 *    itself in a disposable zero-permission subprocess just to read a plugin's own metadata
 *    (`lanrurugi_plugin::pool::PluginPool::plugin_info`) — must not assume `fetch`/file access
 *    are available even if `declared_permissions` asks for them.
 * 2. One `kind`-dependent entry point, matching {@linkcode PluginInfoResult.type}:
 *    - `"metadata"` → `execMetadata(hostArgs): Promise<{@linkcode MetadataResult}>`
 *    - `"login"` → `execLogin(hostArgs): Promise<{@linkcode LoginResult}>`
 *    - `"download"` → `execDownload(hostArgs): Promise<{@linkcode DownloadResult}>`
 *    - `"script"` → `runScript(hostArgs): Promise<unknown>` — legacy's own three Scripts plugins
 *      (FolderToCat/SourceFinder/nHentaiSourceConverter) are reimplemented as native Rust
 *      endpoints instead (`lanrurugi-api::scripts`) rather than shipped as real `.ts` files under
 *      `plugins/script/`, so this entry point currently has no worked example in this repo — a
 *      third-party script plugin uploaded via `POST /plugins/upload` would still be dispatched
 *      through this same generic path, though.
 *
 * A `"download"`-type plugin MAY additionally export `pluginOptions(): {@linkcode
 * PluginOptionsResult}` — synchronous, no side effects, same zero-permission-subprocess call
 * pattern as `pluginInfo()`. Omit it entirely if the plugin has no configurable
 * concurrency/rate-limit/bundling defaults to declare.
 *
 * `lanrurugi-plugin-converter` generates both from a legacy `.pm` file's `plugin_info`/
 * `get_tags`/`do_login`/`provide_url` subs automatically — see
 * `crates/lanrurugi-plugin-converter/src/render.rs`'s own docs, or just run
 * `mise run convert-plugins -- <path-to-legacy-LANraragi-checkout>` (see `scripts/convert-plugins.sh`).
 * Everything below is what that generated code targets; write it by hand only for a genuinely new
 * (non-legacy-derived) plugin.
 *
 * The host-provided globals a plugin body can call (`legacyCompat.*`, `LegacyUserAgent`, ...) are
 * declared in a sibling file, `plugins/legacy-globals.d.ts` — deliberately *not* here, and
 * deliberately *not* an `export`ed symbol from this module (issue #86). `plugins/legacy-globals.d.ts`
 * is a pure global script (no top-level `import`/`export` in it at all), which is what lets
 * `plugins/tsconfig.json` (covering every `.ts` file under `plugins/`) make it ambiently visible
 * to every plugin file with zero boilerplate of its own — no `/// <reference>` line, no import,
 * nothing (this replaced 31 files' worth of copy-pasted `declare global` boilerplate that used to
 * live inline in each one, then briefly a one-line reference directive per file, before landing on
 * this fully zero-touch shape). `dispatcher.ts` (the real runtime home of
 * `globalThis.legacyCompat = {...}`) sits outside `plugins/` and so isn't covered by that
 * project-wide config — it references `plugins/legacy-globals.d.ts` via its own explicit
 * `/// <reference types="..." />` line instead, same as before.
 *
 * The same zero-boilerplate constraint is *also* why {@linkcode MetadataHostArgs},
 * {@linkcode LoginHostArgs}, {@linkcode ScriptHostArgs}, and {@linkcode DownloadHostArgs} —
 * despite being genuinely part of this SDK's protocol contract, not host-implementation
 * plumbing — live in `plugins/legacy-globals.d.ts` too, not here (issue #86's later type-
 * unification pass). `lanrurugi-plugin-converter`'s generated code narrows each entry point's own
 * local info-hash interface with `extends Pick<AuthoritativeInterface, "key1" | "key2" | ...>`
 * against these exact four (`render.rs`'s `authoritative_host_args_interface`) — a real symbol
 * reference a generated plugin file needs to resolve with zero `import`/reference of its own, the
 * same constraint that keeps `Legacy*`/`legacyCompat` out of this `export`-bearing module. Every
 * *other* protocol type below (results, `PluginInfoResult`, permissions, ...) has no such
 * constraint — nothing generated ever needs to reference them by name inside a plugin file's own
 * source, only this module's own callers (`dispatcher.ts`, `deno doc`, a future Rust-side
 * `protocol.rs` comparison) do, so they stay right here as ordinary `export`s.
 *
 * `deno doc` this file together with `dispatcher.ts` (exactly what `mise run plugin-sdk-docs`
 * does) to get the protocol-type half in one place — `plugins/legacy-globals.d.ts`'s own host
 * globals aren't currently included in that generated doc output (a plain `deno doc` invocation
 * against a global script produces nothing useful; folding it in would need its own follow-up).
 * `deno doc --json` (same two input files) is also confirmed to work, producing structured
 * per-symbol output (JSDoc text, resolved `tsType`, source `location`) that's a more natural fit
 * for a future documentation site's own Markdown generation (issue #52) than the fixed
 * pre-rendered HTML bundle `--html` produces — the one thing to know before wiring that up:
 * `--json`'s `nodes` keys and every symbol's `location.filename` are absolute `file://` paths on
 * whatever machine generated them, so a conversion step needs to strip/remap those before the
 * output is portable to another machine or CI.
 *
 * @module
 */

/** The four plugin categories the host recognizes (`lanrurugi-api::plugins::PLUGIN_CATEGORIES`) —
 * also the fixed set of subdirectories under `plugins/` (`metadata/`, `login/`, `download/`,
 * `script/`), and the value `upload_plugin` trusts *only* from a freshly-uploaded plugin's own
 * `pluginInfo()` response (never a client-supplied category) to decide where to file it. */
export type PluginKind = "metadata" | "login" | "download" | "script";

/** One entry in {@linkcode PluginInfoResult.parameters} — a user-configurable setting shown in
 * the plugin's settings UI. The host persists one value per declared parameter (`GET`/
 * `PUT /plugins/settings?namespace=...`, `lanrurugi-api::plugins::plugin_settings_key`, Redis
 * field `customargs` — matches legacy's own `LRR_PLUGIN_<NS>` hash and JSON-array encoding
 * exactly) and passes the whole array back on every call, positionally matching this array
 * (`parameters[0]`'s saved value is `hostArgs.customargs[0]`, etc. — see
 * {@linkcode MetadataHostArgs.customargs}). Distinct from {@linkcode
 * PluginInfoResult.oneshot_arg}, which is a single value typed fresh into a "run once" dialog each
 * time, not persisted here. */
export interface PluginParameter {
  name: string;
  description: string;
  required?: boolean;
  /** Matches legacy's own `param.type` (`~/LANraragi/templates/plugins.html.tt2`'s `SWITCH
   * param.type` — `'string'` → a text input, `'bool'` → a checkbox/switch, `'int'` → a number
   * input). Missing entirely before this — every converted plugin's parameters rendered as a
   * plain, unstyled text input regardless of their real semantics, including boolean toggles like
   * `plugins/metadata/mems.ts`'s own 3 params (real legacy `MEMS.pm` declares all 3 as `type =>
   * 'bool'`, rendered as ON/OFF switches — a plain blank text box here was a real, visible
   * mismatch). Legacy's own unmatched-`SWITCH` fallback renders a `type="color"` picker (its
   * source literally comments this default "ayy lmao" — an unintentional quirk, not a real 4th
   * parameter kind any real plugin in this corpus uses); this SDK deliberately defaults absent
   * `type` to `'string'` instead, since every one of this corpus's 31 plugins with declared
   * parameters had none of this field before now and were already correctly rendered/used as plain
   * text inputs — matching legacy's silly color-picker default would have turned every one of
   * those already-working parameters into a broken control instead of leaving them unaffected. */
  type?: "string" | "bool" | "int";
}

/** What a plugin is allowed to touch — enforced by starting its Deno subprocess with exactly the
 * matching `--allow-net`/`--allow-read`/`--allow-write` flags and nothing wider
 * (`lanrurugi_plugin::pool::Worker::spawn`), constitution Principle IV. Declaring less than a
 * plugin actually needs fails loudly at the call site (a denied `fetch`/file op throws); there is
 * no way to request "read access to one specific file" here beyond {@linkcode
 * PluginInfoResult.sidecar_files}, which the host resolves itself rather than granting real
 * filesystem access for. */
export interface DeclaredPermissions {
  /** Exact hostnames this plugin's `fetch()` calls are allowed to reach — verify every one
   * against a real URL literal in the plugin's own source before shipping it; an empty array
   * means no network access at all. */
  net: string[];
  read: boolean;
  write: boolean;
}

/** `pluginInfo()`'s return shape — matches `lanrurugi_plugin::protocol::PluginInfo` field for
 * field (that's the Rust-side deserialization target every one of these fields is verified
 * against). Fields below `login_from` are display-only, answering `GET /plugins/{type}` with the
 * shape existing UI/tooling expects; they're not part of `contracts/plugin-protocol.md`'s minimal
 * wire contract. */
export interface PluginInfoResult {
  /** Stable machine identifier — also this plugin's filename without the `.ts` extension once
   * installed (`metadata/ehentai.ts` → namespace `metadata/ehentai`), and the Redis key suffix
   * its one configurable `arg` value is stored under. */
  namespace: string;
  type: PluginKind;
  parameters: PluginParameter[];
  declared_permissions: DeclaredPermissions;
  /** Namespace of another `"login"`-type plugin to run *fresh before every single call* to this
   * one (never a cached session) — mirrors legacy's `exec_login_plugin`
   * (`~/LANraragi/lib/LANraragi/Model/Plugins.pm:107-135`). Omit entirely for a plugin that needs
   * no login, or that is itself a `"login"`-type plugin (which never chases another login). A
   * failed login attempt only logs a warning; this plugin's own call still goes ahead, just
   * without `hostArgs.user_agent_cookies` populated. */
  login_from?: string;
  name: string;
  author: string;
  description: string;
  version: string;
  icon?: string;
  /** Prompt text for the single free-text value a user types into this plugin's "run once"
   * dialog (as opposed to a saved settings-page {@linkcode PluginParameter}) — e.g. a URL or
   * gallery ID for a one-shot "copy tags from this other archive" action. Omit if this plugin has
   * no such one-shot action. */
  oneshot_arg?: string;
  /** A regex (JS `RegExp` source, no delimiters, e.g. `"pixiv\\.net"`) matched case-insensitively
   * against a full candidate URL to decide whether this plugin should handle it — used by the
   * Upload page's URL queue to group pasted URLs by download plugin, and, for a metadata plugin,
   * to find the one matching plugin for a "fetch metadata preview" action against a bare URL
   * (before any archive exists). Evaluated entirely client-side; the host never matches against
   * this itself. Omit if this plugin has no meaningful URL-based routing (e.g. a script/login
   * plugin, or a metadata/download plugin with no single well-known source domain). */
  url_pattern?: string;
  /** Sidecar metadata filenames this plugin wants read out of the archive it's currently
   * processing (basename suffixes, e.g. `"ComicInfo.xml"`, `"info.json"`) — `lanrurugi-
   * plugin-converter` populates this automatically from every `is_file_in_archive(...)` call it
   * finds in a converted plugin's source. The host resolves each name against the current archive
   * *before* calling `execMetadata` and hands back whatever it found as {@linkcode
   * MetadataHostArgs.sidecar_files}: file *content*, never a path — this plugin itself never gets
   * real filesystem access for this (constitution Principle IV). */
  sidecar_files?: string[];
}

/** A plugin-authored error — `error_code` doubles as an i18n lookup key (the frontend's
 * `apps/frontend/src/i18n/locales/*.json` use literal English sentences as keys, not symbolic
 * codes; a missing translation falls back to rendering `error_code` itself verbatim, i18next's
 * own default behavior). Write `error_code` as a natural, stable English phrase that does NOT
 * embed any dynamic value (a file path, a URL, an HTTP status code, ...) — those go in
 * {@linkcode data} instead, so the same `error_code` can be looked up and translated regardless
 * of which specific file/URL/status triggered it. E.g. for "Could not open foo.json!", use
 * `error_code: "Could not open file"` + `data: {filepath: "foo.json"}`, not the interpolated
 * string itself as the code. */
export interface PluginError {
  error_code: string;
  /** Interpolation params for the translated string — only strings/numbers (filenames, URLs,
   * other messages, page counts, HTTP status codes: everything this SDK's real plugin corpus
   * actually needs to interpolate), never a richer/nested value. */
  data?: Record<string, string | number>;
}

/** Throw this (instead of a plain `throw new Error(...)`) to report a {@linkcode PluginError}
 * through an exception rather than a `return {error}` — used for the exact same "expected,
 * semantic failure" cases {@linkcode MetadataResult.error} documents, just from a code path
 * that's more naturally a `throw` (deep inside a helper, a caught-and-rethrown parse failure,
 * etc.) than a `return`. The dispatcher (`dispatcher.ts`'s `handleRequest`) special-cases this
 * class specifically — an unexpected `throw` of anything else (a genuine bug, a network library's
 * own exception type) still degrades to today's generic unstructured-fault handling, since an
 * arbitrary caught value has no stable translatable shape to extract `error_code`/`data` from. */
export class PluginErrorException extends Error {
  constructor(
    public error_code: string,
    public data?: Record<string, string | number>,
  ) {
    super(error_code);
  }
}

/** `execMetadata`'s return shape. This is the real, verified-against-every-converted-plugin
 * contract: every one of this repo's 21 converted `Metadata/*.pm` plugins (and every legacy `.pm`
 * source they came from) returns exactly `(tags => ..., title => ...)` from its own `get_tags`
 * sub, and `lanrurugi-api::plugins::use_plugin_sync`/`use_plugin_async` currently forward a
 * plugin's JSON response to the API's own `data` field completely unchanged — no field renaming
 * happens on the host side today. (Legacy's own `~/LANraragi/lib/LANraragi/Model/Plugins.pm` does
 * rename a plugin's `tags` to `new_tags` before its real HTTP API sees it, matching
 * `~/LANraragi/tools/openapi.yaml`'s documented response shape — this repo deliberately doesn't
 * mirror that rename, since nothing on the host or frontend side depends on the `new_tags` name;
 * `crates/lanrurugi-plugin/samples/sample-metadata-plugin.ts` and every real call site use `tags`
 * to match.) */
export interface MetadataResult {
  tags?: string;
  title?: string;
  /** Present instead of `tags`/`title` on failure (e.g. a filename plugin whose regex didn't
   * match) — a structured {@linkcode PluginError}, not thrown, so the host can report it without
   * the whole `use_plugin` call failing. */
  error?: PluginError;
}

/** `execLogin`'s return shape. Only `cookies` is ever read by the host
 * (`lanrurugi-api::plugins::with_login_cookies`), which folds it into the *next*
 * metadata/download call's {@linkcode MetadataHostArgs.user_agent_cookies} — every other property
 * a real `legacyCompat.userAgent()` instance exposes is a function, which `JSON.stringify` silently
 * drops crossing the dispatcher's newline-delimited-JSON boundary (see `dispatcher.ts`'s own
 * `LegacyUserAgent.cookies` docs), so returning the whole `ua` object works, but only this field
 * survives. */
export interface LoginResult {
  cookies?: LegacyCookie[];
  error?: PluginError;
}

/** `runScript`'s return shape — deliberately a loose bag of fields (mirrors legacy's own
 * `run_script` subs, each of which returns a different ad-hoc hash with no shared shape across the
 * three real Scripts plugins) rather than one rigid interface every script must conform to.
 * `total`/`id`/`error` are `SourceFinder.pm`'s own fields; `modified`/`updates` are
 * `nHentaiSourceConverter.pm`'s. */
export interface ScriptResult {
  /** `1` if a match was found, `0` otherwise (`urlfinder`). */
  total?: number;
  /** The matching archive's ID, when {@linkcode total} is `1` (`urlfinder`). */
  id?: string;
  error?: PluginError;
  /** Count of archives this call changed (`nhsrcconv`). */
  modified?: number;
  /** This call's intended tag rewrites — the host applies each one (same shape as
   * {@linkcode ScriptHostArgs.archives}) after `runScript` returns; a plugin never writes storage
   * directly (`nhsrcconv`). */
  updates?: ScriptArchiveTags[];
  /** One static Category to create per discovered subfolder — the host creates each one after
   * `runScript` returns, same read-compute-return-write-to-host split as {@linkcode updates}
   * (`foldertocat`). */
  categories_to_create?: { name: string; archive_ids: string[] }[];
  /** Whether to delete every existing static (non-dynamic) category before creating the new ones
   * — echoes back this call's own `customargs[0]`, since the host applies the deletion itself
   * (`foldertocat`). */
  delete_old_categories?: boolean;
  /** Wall-clock milliseconds `runScript` itself spent walking the directory tree and computing
   * groupings — reported back for a direct comparison against the equivalent native Rust endpoint
   * (`POST /database/scripts/subfolders-to-categories`'s own `elapsed_ms`), which does the
   * identical job. Not a generic field every script plugin is expected to fill in (`foldertocat`
   * only). */
  elapsed_ms?: number;
}

/** One resource to fetch, as part of {@linkcode DownloadResult.downloads} — see that field's own
 * docs. `specs/005-download-plugin-progress/contracts/plugin-download-protocol.md` is the wire
 * contract this mirrors field-for-field. */
export interface DownloadRequest {
  /** The real, directly-fetchable resource URL. */
  url: string;
  /** Defaults to `"GET"` when absent — every real corpus plugin (and legacy LANraragi's own
   * `Model::Upload.pm::download_url`) only ever uses GET, but other methods remain representable
   * for a future plugin that needs one. */
  method?: string;
  /** Extra request headers for this specific resource (e.g. Pixiv's anti-hotlink `Referer`
   * header) — absent means no extra headers beyond whatever the host's own downloader sends by
   * default. */
  headers?: Record<string, string>;
  /** A plugin-suggested filename, used only as a fallback when the real HTTP response has no
   * (or an unparseable) `Content-Disposition` header. */
  filename_hint?: string;
}

/** `execDownload`'s return shape — verified against every real legacy `Download/*.pm`'s own
 * `provide_url` sub. The host performs the real byte-level HTTP transfer itself now (Rust-side
 * streaming download — see `specs/005-download-plugin-progress/plan.md`'s Summary for why: this
 * is what makes real progress reporting, per-domain concurrency limiting, and rate limiting
 * possible at all), rather than treating a plugin's result as an opaque already-downloaded blob.
 *
 * Exactly one of {@linkcode downloads}, {@linkcode file_path}, or {@linkcode error} is present in
 * a given result; a `downloads` array, if present, has at least one element. */
export interface DownloadResult {
  /** One element = a single-file download (e.g. Chaika/EHentai-style plugins, which resolve one
   * real download URL); more than one element = a multi-resource download (e.g. Pixiv's per-page
   * images), assembled per the plugin's {@linkcode PluginOptionsResult.bundle_as_archive}
   * setting. */
  downloads?: DownloadRequest[];
  /** Pre-existing fallback escape hatch: a plugin that already downloaded/wrote a file itself and
   * hands back a local path. Mutually exclusive with {@linkcode downloads}; does not receive
   * progress/concurrency/rate-limit treatment since the transfer already happened entirely inside
   * the plugin process by the time this returns. */
  file_path?: string;
  error?: PluginError;
}

/** One per-domain concurrency/rate-limit rule, as declared by a plugin's own {@linkcode
 * PluginOptionsResult.domain_rules} default, or as a user's persisted override
 * (`GET`/`PUT /api/plugins/{namespace}/options`) — `specs/005-download-plugin-progress/
 * data-model.md`'s `Domain Rule`. */
export interface DomainRule {
  /** An exact hostname (`"example.com"`) or a wildcard covering subdomains (`"*.example.com"`).
   * Case-insensitive; no port/scheme component. Omit entirely (or use `"*"`) for a general,
   * non-domain-specific fallback rule. An exact-hostname match always takes precedence over a
   * wildcard match, which always takes precedence over the general fallback. */
  pattern?: string;
  /** Maximum simultaneous downloads permitted against a domain matching `pattern`. Absent = no
   * concurrency limit from this rule. */
  max_concurrent?: number;
  /** Maximum transfer speed (bytes/sec) permitted against a domain matching `pattern`. Absent =
   * no rate limit from this rule. */
  max_bytes_per_sec?: number;
  /** Human-readable explanation shown in the settings UI. */
  description?: string;
}

/** `pluginOptions()`'s return shape — a plain additional `export function`, parallel to the
 * mandatory `pluginInfo()` (see this module's own top-level docs). Omitting `pluginOptions()`
 * entirely is valid, and is the expected shape for every non-download plugin, and for a download
 * plugin with nothing to configure — the host then shows no settings UI for that plugin at all
 * (spec FR-015). `specs/005-download-plugin-progress/contracts/plugin-download-protocol.md` is the
 * wire contract this mirrors field-for-field. */
export interface PluginOptionsResult {
  /** The plugin's own declared default {@linkcode DomainRule}s for the domain(s) it targets.
   * Absent/empty means the plugin declares no concurrency/rate-limit defaults of its own — that
   * domain is unmanaged (unlimited) unless a user override exists (spec FR-017: the system itself
   * imposes no default). */
  domain_rules?: DomainRule[];
  /** Only meaningful for a plugin whose `execDownload` can return more than one
   * {@linkcode DownloadResult.downloads} element. Absent for a single-resource-only plugin (no
   * such setting shown at all). */
  bundle_as_archive?: {
    /** The plugin's own declared default (e.g. Pixiv: `true` — its multi-page downloads must ship
     * as one manga archive, not one archive per page). */
    default: boolean;
    /** Human-readable explanation shown in the settings UI. */
    description: string;
  };
  /** Whether starting a download for a URL this plugin handles should overwrite an existing
   * archive that collides by content hash or destination filename, rather than the default safe
   * behavior (reject the new file, keep the existing archive untouched). Omit if this plugin has
   * no opinion — the effective value then falls back to the global `replacedupe` setting. */
  overwrite_on_duplicate?: {
    default: boolean;
    /** Human-readable explanation shown in the settings UI. */
    description: string;
  };
}

