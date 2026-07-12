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
 * `lanrurugi-plugin-converter` generates both from a legacy `.pm` file's `plugin_info`/
 * `get_tags`/`do_login`/`provide_url` subs automatically — see
 * `crates/lanrurugi-plugin-converter/src/render.rs`'s own docs, or just run
 * `mise run convert-plugins -- <path-to-legacy-LANraragi-checkout>` (see `scripts/convert-plugins.sh`).
 * Everything below is what that generated code targets; write it by hand only for a genuinely new
 * (non-legacy-derived) plugin.
 *
 * The host-provided globals a plugin body can call (`perlCompat.*`, `PerlUserAgent`, ...) are
 * documented in `dispatcher.ts` itself, not repeated here — `deno doc` this file together with
 * that one (exactly what `mise run plugin-sdk-docs` does) to get both halves in one place.
 *
 * @module
 */

/** The four plugin categories the host recognizes (`lanrurugi-api::plugins::PLUGIN_CATEGORIES`) —
 * also the fixed set of subdirectories under `plugins/` (`metadata/`, `login/`, `download/`,
 * `script/`), and the value `upload_plugin` trusts *only* from a freshly-uploaded plugin's own
 * `pluginInfo()` response (never a client-supplied category) to decide where to file it. */
export type PluginKind = "metadata" | "login" | "download" | "script";

/** One entry in {@linkcode PluginInfoResult.parameters} — a user-configurable setting shown in
 * the plugin's settings UI. Only the *first* declared parameter is currently wired up: the host
 * passes exactly one generic string value per call (`hostArgs.arg`, see
 * {@linkcode MetadataHostArgs.arg}) — a plugin declaring more than one parameter gets every extra
 * one bound to that same single value (`lanrurugi-plugin-converter` flags this with a conversion
 * warning; `lanrurugi-api::plugins::plugin_settings_key` is the storage side of the one value that
 * does work). */
export interface PluginParameter {
  name: string;
  description: string;
  required?: boolean;
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
  /** Sidecar metadata filenames this plugin wants read out of the archive it's currently
   * processing (basename suffixes, e.g. `"ComicInfo.xml"`, `"info.json"`) — `lanrurugi-
   * plugin-converter` populates this automatically from every `is_file_in_archive(...)` call it
   * finds in a converted plugin's source. The host resolves each name against the current archive
   * *before* calling `execMetadata` and hands back whatever it found as {@linkcode
   * MetadataHostArgs.sidecar_files}: file *content*, never a path — this plugin itself never gets
   * real filesystem access for this (constitution Principle IV). */
  sidecar_files?: string[];
}

/** A cookie exactly as `Mojo::Cookie::Response->new(name => ..., value => ..., domain => ...,
 * path => ...)` shapes it in the legacy Perl source this was converted from — see `dispatcher.ts`'s
 * own `PerlCookie` docs. */
export interface PerlCookieLike {
  name: string;
  value: string;
  domain: string;
  path: string;
}

/** `hostArgs` for `execMetadata`. In practice a converted plugin's generated entry point binds
 * the *whole* object to one variable (`let lrr_info = hostArgs as Record<string, any>;`) and reads
 * individual fields off it by name — this interface exists so a hand-written plugin (or this doc)
 * has something concrete to check field names against. */
export interface MetadataHostArgs {
  /** Present when the call was made against a real archive (`GET /plugins/use_plugin?id=...`);
   * absent for a settings-page "test this plugin" dry run. */
  archive_id?: string;
  /** The plugin's *one* configurable value — see {@linkcode PluginParameter}'s own docs on why
   * only the first declared parameter is ever actually reachable here. */
  arg: string;
  /** The archive's on-disk path — fetched by the host once per call
   * (`lanrurugi-api::plugins::use_plugin_sync`) so a filename-deriving plugin (e.g. "Filename
   * Parsing") doesn't need its own round trip back into the API just to resolve an ID to a path. */
  file_path?: string;
  /** Content of every file listed in this plugin's own {@linkcode
   * PluginInfoResult.sidecar_files}, keyed by that same filename — present (possibly empty) only
   * when `sidecar_files` was non-empty; a missing/unreadable/non-UTF-8 file is just absent from
   * this map rather than failing the whole call. */
  sidecar_files?: Record<string, string>;
  /** Populated only when this plugin (or the one it declares via {@linkcode
   * PluginInfoResult.login_from}) needs a logged-in session — see {@linkcode LoginResult}'s own
   * docs for where this comes from. A converted plugin's entry point rehydrates this into a real
   * `perlCompat.userAgent()` instance before its body runs (`render.rs`'s
   * `render_user_agent_hydration_preamble`). */
  user_agent_cookies?: PerlCookieLike[];
}

/** `execMetadata`'s return shape. This is the real, verified-against-every-converted-plugin
 * contract: every one of this repo's 21 converted `Metadata/*.pm` plugins (and every legacy `.pm`
 * source they came from) returns exactly `(tags => ..., title => ...)` from its own `get_tags`
 * sub, and `lanrurugi-api::plugins::use_plugin_sync`/`use_plugin_async` currently forward a
 * plugin's JSON response to the API's own `data` field completely unchanged — no field renaming
 * happens on the host side today.
 *
 * **Known inconsistency, not yet resolved:** legacy's own `~/LANraragi/lib/LANraragi/Model/
 * Plugins.pm` (line ~291) *does* rename a plugin's `tags` to `new_tags` before that value reaches
 * legacy's real HTTP API (matching `~/LANraragi/tools/openapi.yaml`'s documented response shape).
 * This repo's own `lanrurugi_plugin::protocol::MetadataResult` struct and
 * `crates/lanrurugi-plugin/samples/sample-metadata-plugin.ts` were written against that
 * `new_tags`-shaped contract instead, and neither is actually exercised by the real Rust API path
 * (`MetadataResult` is never deserialized anywhere; the sample plugin is only driven directly
 * through `PluginPool` in `pool.rs`'s own tests, never through `lanrurugi-api::plugins`). Until one
 * side is fixed, trust *this* interface for anything that goes through the real `/plugins/
 * use_plugin` HTTP path — it's the one verified against actual converted-from-legacy plugin
 * behavior. */
export interface MetadataResult {
  tags?: string;
  title?: string;
  /** Present instead of `tags`/`title` on failure (e.g. a filename plugin whose regex didn't
   * match) — a plain string message, not thrown, so the host can report it without the whole
   * `use_plugin` call failing. */
  error?: string;
}

/** `hostArgs` for `execLogin` — always just the plugin's own one configurable value (credentials,
 * typically), never an `archive_id`/`file_path` (a login plugin never runs against a specific
 * archive). */
export interface LoginHostArgs {
  arg: string;
}

/** `execLogin`'s return shape. Only `cookies` is ever read by the host
 * (`lanrurugi-api::plugins::with_login_cookies`), which folds it into the *next*
 * metadata/download call's {@linkcode MetadataHostArgs.user_agent_cookies} — every other property
 * a real `perlCompat.userAgent()` instance exposes is a function, which `JSON.stringify` silently
 * drops crossing the dispatcher's newline-delimited-JSON boundary (see `dispatcher.ts`'s own
 * `PerlUserAgent.cookies` docs), so returning the whole `ua` object works, but only this field
 * survives. */
export interface LoginResult {
  cookies?: PerlCookieLike[];
  error?: string;
}

/** `hostArgs` for `execDownload` — the user-supplied URL/ID to download, plus an optional target
 * category (`POST /download_url?url=...&catid=...`). Unlike `execMetadata` there's no
 * `archive_id`/`file_path`: nothing has been catalogued yet at this point, that's the whole job
 * this entry point exists to do. */
export interface DownloadHostArgs {
  arg: string;
  category?: string;
  user_agent_cookies?: PerlCookieLike[];
}

/** `execDownload`'s return shape — verified against every real legacy `Download/*.pm`'s own
 * `provide_url` sub, all of which return exactly `(download_url => ...)` on success or
 * `(error => ...)` on failure. The host currently just stores this as an opaque background-job
 * result (`lanrurugi-api::plugins`'s `use_plugin_async`-style `tokio::spawn` + `jobs.finish`) — no
 * `Download/*.pm` file has been converted yet (`mise run convert-plugins -- <legacy-checkout>
 * download` does this). */
export interface DownloadResult {
  download_url?: string;
  error?: string;
}
