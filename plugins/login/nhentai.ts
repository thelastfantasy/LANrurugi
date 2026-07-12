// Converted from ~/LANraragi/lib/LANraragi/Plugin/Login/nHentai.pm via
// `lanrurugi-plugin-converter` (`cargo run -p lanrurugi-plugin-converter -- convert ...`).
// Verified: `deno check` passes with zero errors on this file.

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
    namespace: "nhapiauth",
    type: "login" as const,
    parameters: [
      { name: "param1", description: "API Key", required: false },
    ],
    // Only ever sets an Authorization header (via `on('start', ...)`) + User-Agent on a
    // `perlCompat.userAgent()` instance and returns it (verified against every line of
    // ~/LANraragi/lib/LANraragi/Plugin/Login/nHentai.pm) — `net` is still declared with the real
    // target domain (constitution Principle IV: declare what the *capability* is actually for,
    // not just what this file's own code directly calls; the nHentai metadata plugin's API calls
    // all target nhentai.net).
    declared_permissions: { net: ["nhentai.net"], read: false, write: false },
    name: "nHentai",
    author: "Guerra24 (ported)",
    description: "Authenticates the nHentai API using an API Key. You can generate one in your profile's settings.",
    version: "1.0",
  };
}


export async function execLogin(hostArgs: Record<string, unknown>) {
  //    # Login plugins only receive the parameters entered by the user.

  // (shift) discarded positional arg — legacy Perl-OOP invocant/first @_ slot
  let key = hostArgs.arg as string;
  let logger = perlCompat.getLogger("nHentai API Auth", "plugins");
  let ua = perlCompat.userAgent();
  let version_info = perlCompat.getVersion();
  let version = version_info["version"];
  let homepage = version_info["homepage"];
  ua.transactor.name(`LANraragi/${version} (+${homepage})`);
  if (key) {
    logger.info(`API Key provided (${key})!`);
    ua.on("start", (ua: any, tx: any) => {tx.req.headers.header("Authorization", `Key ${key}`);});
  } else {
    logger.info("No API Key provided");
  }
  return ua;
}
