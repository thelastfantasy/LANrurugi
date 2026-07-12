// Converted from ~/LANraragi/lib/LANraragi/Plugin/Login/Fakku.pm via
// `lanrurugi-plugin-converter` (`cargo run -p lanrurugi-plugin-converter -- convert ...`).
// Verified: `deno check` passes with zero errors on this file.
//
// Known limitation: this plugin declares 2 custom parameters (fakku_sid cookie / custom
// User-Agent), but the host currently passes only one generic `arg` string per call — both end up
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
    namespace: "fakkulogin",
    type: "login" as const,
    parameters: [
      { name: "param1", description: "fakku_sid cookie value", required: false },
      { name: "param2", description: "Useragent value", required: false },
    ],
    // Only ever sets a cookie + User-Agent on a `perlCompat.userAgent()` instance and returns it
    // (verified against every line of ~/LANraragi/lib/LANraragi/Plugin/Login/Fakku.pm) — `net` is
    // still declared with the real target domain (constitution Principle IV: declare what the
    // *capability* is actually for, not just what this file's own code directly calls).
    declared_permissions: { net: ["fakku.net"], read: false, write: false },
    name: "Fakku",
    author: "Nodja, Nixis198 (ported)",
    description: "Handles login to FAKKU. If the FAKKU metadata plugin stops working, update your 'fakku_sid' cookie and add your own Useragent.",
    version: "0.2",
  };
}


export async function execLogin(hostArgs: Record<string, unknown>) {
  // (shift) discarded positional arg — legacy Perl-OOP invocant/first @_ slot
  let fakku_sid = hostArgs.arg as string;
  let useragentcustom = hostArgs.arg as string;
  let useragent = undefined;
  let useragentdefault = 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/137.0.0.0 Safari/537.36';
  let logger = perlCompat.getLogger("Fakku Login", "plugins");
  let ua = perlCompat.userAgent();
  //    # If the user didn't provide a useragent use the default one

  if (useragentcustom === "") {
    useragent = useragentdefault;
  } else {
    useragent = useragentcustom;
  }
  if (fakku_sid !== "" && useragent !== "") {
    logger.info(`Cookie provided (${fakku_sid})!`);
    ua.cookie_jar.add({name: 'fakku_sid', value: fakku_sid, domain: 'fakku.net', path: '/'});
    logger.debug(`Using Useragent: (${useragent})!`);
    ua.transactor.name(useragent);
  } else {
    logger.info("No cookies provided, returning blank UserAgent.");
  }
  return ua;
}
