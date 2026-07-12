// Sample metadata plugin exercising the dispatcher protocol (User Story 4, T063).
//
// Declares a `net` permission for a single fake host, so `lanrurugi-plugin::pool` grants exactly
// `--allow-net=metadata.example.invalid` to this plugin's worker and nothing else — demonstrating
// the permission model end to end (constitution Principle IV).

interface PluginInfoResult {
  namespace: string;
  type: "metadata" | "login" | "download";
  parameters: Array<{ name: string; description: string; required: boolean }>;
  declared_permissions: { net: string[]; read: boolean; write: boolean };
  name: string;
  author: string;
  description: string;
  version: string;
}

export function pluginInfo(): PluginInfoResult {
  return {
    namespace: "sample-metadata-plugin",
    type: "metadata",
    parameters: [],
    declared_permissions: {
      net: ["metadata.example.invalid"],
      read: false,
      write: false,
    },
    name: "Sample Metadata Plugin",
    author: "LANrurugi",
    description: "Demonstrates the Deno plugin protocol end to end (User Story 4).",
    version: "1.0.0",
  };
}

interface ExecMetadataArgs {
  archive_id: string;
  first_page_path?: string;
  parameters?: Record<string, unknown>;
}

interface ExecMetadataResult {
  new_tags?: string;
  title?: string;
  summary?: string;
}

// A real plugin would `fetch()` a metadata site here. This sample deliberately hits an
// unreachable, permission-scoped host so the timeout/failure-isolation path
// (quickstart.md §4's "point the plugin at an unreachable endpoint") is exercisable without any
// real network dependency, while still proving the plugin can *only* reach the host it declared.
export async function execMetadata(args: ExecMetadataArgs): Promise<ExecMetadataResult> {
  try {
    await fetch("https://metadata.example.invalid/lookup", { signal: AbortSignal.timeout(5000) });
  } catch {
    // Unreachable by design in this sample; fall through to a deterministic canned result so the
    // happy-path (enrichment actually populating tags) is still demonstrable.
  }
  return {
    new_tags: `source:sample,archive:${args.archive_id}`,
    summary: "Enriched by sample-metadata-plugin.",
  };
}
