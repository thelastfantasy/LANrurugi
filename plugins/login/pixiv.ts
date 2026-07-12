// Converted from ~/LANraragi/lib/LANraragi/Plugin/Login/Pixiv.pm via
// `lanrurugi-plugin-converter` (`cargo run -p lanrurugi-plugin-converter -- convert ...`).
// Verified: `deno check` passes with zero errors on this file.
//
// Known limitation: this plugin declares 2 custom parameters (custom User-Agent / PHPSESSID
// cookie), but the host currently passes only one generic `arg` string per call — both end up
// bound to the *same* value below, which is wrong (each needs its own real value). This plugin
// cannot correctly log in until the host protocol grows multi-parameter support;
// `lanrurugi-plugin-converter` flagged this as a conversion warning.

declare global {
  interface PerlTransaction {
    req: { headers: { header(name: string, value: string): void } };
  }
  interface PerlUserAgent {
    cookie_jar: { add(cookie: { name: string; value: string; domain: string; path: string }): void };
    max_redirects(n: number): PerlUserAgent;
    transactor: { name(value: string): void };
    on(event: "start", handler: (ua: PerlUserAgent, tx: PerlTransaction) => void): void;
    get(url: string): Promise<{ result: PerlHttpResult }>;
    post(url: string, kind: "form" | "json", data: Record<string, string> | Record<string, unknown>): Promise<{ result: PerlHttpResult }>;
    cookies: { name: string; value: string; domain: string; path: string }[];
  }
  interface PerlHttpResult {
    body: string;
    code: number;
    readonly dom: PerlDomNode;
    readonly json: unknown;
  }
  interface PerlLogger {
    debug(msg: string): void;
    info(msg: string): void;
    warn(msg: string): void;
    error(msg: string): void;
  }
  interface PerlDomNode {
    text: string;
    attr(name: string): string | undefined;
    parent: PerlDomNode | undefined;
    at(selector: string): PerlDomNode | undefined;
    find(selector: string): PerlDomNode[] & { each<T>(fn: (node: PerlDomNode, index: number) => T): T[] };
    toString(): string;
  }
  // deno-lint-ignore no-var
  var perlCompat: {
    reverse<T>(list: readonly T[]): T[];
    chomp(s: string): string;
    sprintf(format: string, ...args: unknown[]): string;
    userAgent(): PerlUserAgent;
    getLogger(name: string, category: string): PerlLogger;
    htmlUnescape(s: string): string;
    parseHtml(markup: string, xml?: boolean): PerlDomNode;
    sleep(seconds: number): Promise<void>;
    getVersion(): { version: string; homepage: string };
  };
}

export function pluginInfo() {
  return {
    namespace: "pixivlogin",
    type: "login" as const,
    parameters: [
      { name: "param1", description: "Browser UserAgent (Default is 'Mozilla/5.0')", required: false },
      { name: "param2", description: "Cookie (PHP session ID)", required: false },
    ],
    // Only ever sets a cookie + User-Agent on a `perlCompat.userAgent()` instance and returns it
    // (verified against every line of ~/LANraragi/lib/LANraragi/Plugin/Login/Pixiv.pm) — `net` is
    // still declared with the real target domain (constitution Principle IV: declare what the
    // *capability* is actually for, not just what this file's own code directly calls).
    declared_permissions: { net: ["pixiv.net"], read: false, write: false },
    name: "Pixiv Login",
    author: "psilabs-dev (ported)",
    description: "Handles login to Pixiv. See https://github.com/Nandaka/PixivUtil2/wiki for how to obtain the cookie.",
    version: "0.1",
  };
}


export async function execLogin(hostArgs: Record<string, unknown>) {
  //    # Login plugins only receive the parameters entered by the user.

  // (shift) discarded positional arg — legacy Perl-OOP invocant/first @_ slot
  let useragent = hostArgs.arg as string;
  let php_session_id = hostArgs.arg as string;
  return get_user_agent(useragent, php_session_id);
}

function get_user_agent(...args: any[]) {
  let [useragent, php_session_id] = args.slice(0);
  //    # assign default user agent.

  if (useragent === '') {
    useragent = "Mozilla/5.0";
  }
  let logger = perlCompat.getLogger("Pixiv Login", "plugins");
  let ua = perlCompat.userAgent();
  if (useragent !== "" && php_session_id !== "") {
    //        # assign user agent.

    ua.transactor.name(useragent);
    //        # add cookie

    ua.cookie_jar.add({name: "PHPSESSID", value: php_session_id, domain: 'pixiv.net', path: '/'});
  } else {
    logger.info("No cookies provided, returning blank UserAgent.");
  }
  return ua;
}
