// Sample download plugin exercising the `pluginOptions()` protocol addition
// (specs/005-download-plugin-progress). Declares a `net` permission for a single fake host, same
// pattern as sample-metadata-plugin.ts.

interface PluginInfoResult {
  namespace: string;
  type: "metadata" | "login" | "download";
  parameters: Array<{ name: string; description: string; required: boolean }>;
  declared_permissions: { net: string[]; read: boolean; write: boolean };
  name: string;
  author: string;
  description: string;
  version: string;
  // Precise trigger condition (real dispatch matches a full URL against this).
  url_pattern?: string;
  // Bare domain(s) this plugin owns, for domain-ownership lookups only — see
  // `PluginInfoResult.domain_match`'s own docs in `plugin-sdk.ts`.
  domain_match?: string[];
}

export function pluginInfo(): PluginInfoResult {
  return {
    namespace: "sample-download-plugin",
    type: "download",
    parameters: [],
    declared_permissions: {
      net: ["download.example.invalid"],
      read: false,
      write: false,
    },
    name: "Sample Download Plugin",
    author: "LANrurugi",
    description: "Demonstrates the pluginOptions() protocol addition end to end.",
    version: "1.0.0",
    url_pattern: "download\\.example\\.invalid",
    domain_match: ["download.example.invalid"],
  };
}

interface DomainRule {
  pattern?: string;
  max_concurrent?: number;
  max_bytes_per_sec?: number;
  description?: string;
}

interface PluginOptionsResult {
  domain_rules?: DomainRule[];
  bundle_as_archive?: { default: boolean; description: string };
}

export function pluginOptions(): PluginOptionsResult {
  return {
    domain_rules: [
      {
        pattern: "*.download.example.invalid",
        max_concurrent: 2,
        description: "Limit simultaneous downloads from this sample host",
      },
    ],
    bundle_as_archive: {
      default: true,
      description: "Combine all downloaded resources into a single archive",
    },
  };
}

interface ExecDownloadArgs {
  arg: string;
  category?: string;
}

export async function execDownload(args: ExecDownloadArgs) {
  return { downloads: [{ url: args.arg }] };
}
