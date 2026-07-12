// Converted from ~/LANraragi/lib/LANraragi/Plugin/Login/EHentai.pm via
// `lanrurugi-plugin-converter` (`cargo run -p lanrurugi-plugin-converter -- convert ...`).
// Verified: `deno check` passes with zero errors on this file.
//
// Known limitation: this plugin declares 4 custom parameters (ipb_member_id/ipb_pass_hash/star/
// igneous cookies), but the host currently passes only one generic `arg` string per call — all
// four end up bound to the *same* value below, which is wrong (each needs its own real cookie
// value). This plugin cannot correctly log in until the host protocol grows multi-parameter
// support; `lanrurugi-plugin-converter` flagged this as a conversion warning.

declare global {
  interface PerlUserAgent {
    cookie_jar: { add(cookie: { name: string; value: string; domain: string; path: string }): void };
    max_redirects(n: number): PerlUserAgent;
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
  };
}

export function pluginInfo() {
  return {
    namespace: "ehlogin",
    type: "login" as const,
    parameters: [
      { name: "param1", description: "ipb_member_id cookie", required: false },
      { name: "param2", description: "ipb_pass_hash cookie", required: false },
      { name: "param3", description: "star cookie (optional, if present you can view fjorded content without exhentai)", required: false },
      { name: "param4", description: "igneous cookie(optional, if present you can view exhentai without Europe and America IP)", required: false },
    ],
    // This plugin only ever sets cookies on a `perlCompat.userAgent()` instance and returns it —
    // it never itself calls `->get()`/`->post()` (verified against every line of
    // ~/LANraragi/lib/LANraragi/Plugin/Login/EHentai.pm), so it makes no network requests of its
    // own. `net` is still declared with the real domains the cookies target (e-hentai.org,
    // exhentai.org, forums.e-hentai.org) rather than left empty, since the returned `UserAgent`
    // is handed back to the host and used for real requests to exactly these hosts by whichever
    // metadata/download plugin's `login_from` names this one (constitution Principle IV: declare
    // what the *capability* is actually for, not just what this file's own code directly calls).
    declared_permissions: {
      net: ["e-hentai.org", "exhentai.org", "forums.e-hentai.org"],
      read: false,
      write: false,
    },
    name: "E-Hentai",
    author: "Difegue (ported)",
    description: "Handles login to E-H. If you have an account that can access fjorded content or exhentai, adding the credentials here will make more archives available for parsing.",
    version: "2.3",
  };
}


export async function execLogin(hostArgs: Record<string, unknown>) {
  //    # Login plugins only receive the parameters entered by the user.

  // (shift) discarded positional arg — legacy Perl-OOP invocant/first @_ slot
  let ipb_member_id = hostArgs.arg as string;
  let ipb_pass_hash = hostArgs.arg as string;
  let star = hostArgs.arg as string;
  let igneous = hostArgs.arg as string;
  return get_user_agent(ipb_member_id, ipb_pass_hash, star, igneous);
}

function get_user_agent(...args: any[]) {
  let [ipb_member_id, ipb_pass_hash, star, igneous] = args.slice(0);
  let logger = perlCompat.getLogger("E-Hentai Login", "plugins");
  let ua = perlCompat.userAgent();
  if (ipb_member_id !== "" && ipb_pass_hash !== "") {
    logger.info(`Cookies provided (${ipb_member_id} ${ipb_pass_hash} ${star} ${igneous})!`);
    //        #Setup the needed cookies with both domains

    //        #They should translate to exhentai cookies with the igneous value generated

    ua.cookie_jar.add({name: 'ipb_member_id', value: ipb_member_id, domain: 'exhentai.org', path: '/'});
    ua.cookie_jar.add({name: 'ipb_member_id', value: ipb_member_id, domain: 'e-hentai.org', path: '/'});
    ua.cookie_jar.add({name: 'ipb_pass_hash', value: ipb_pass_hash, domain: 'exhentai.org', path: '/'});
    ua.cookie_jar.add({name: 'ipb_pass_hash', value: ipb_pass_hash, domain: 'e-hentai.org', path: '/'});
    ua.cookie_jar.add({name: 'star', value: star, domain: 'exhentai.org', path: '/'});
    ua.cookie_jar.add({name: 'igneous', value: igneous, domain: 'exhentai.org', path: '/'});
    ua.cookie_jar.add({name: 'star', value: star, domain: 'e-hentai.org', path: '/'});
    ua.cookie_jar.add({name: 'igneous', value: igneous, domain: 'e-hentai.org', path: '/'});
    ua.cookie_jar.add({name: 'ipb_coppa', value: '0', domain: 'forums.e-hentai.org', path: '/'});
    //        #Skips the "offensive warning" screen so that such galleries archive gIDs can be easily retrieved by Download script.

    ua.cookie_jar.add({name: 'nw', value: '1', domain: 'exhentai.org', path: '/'});
    ua.cookie_jar.add({name: 'nw', value: '1', domain: 'e-hentai.org', path: '/'});
  } else {
    logger.info("No cookies provided, returning blank UserAgent.");
  }
  return ua;
}
