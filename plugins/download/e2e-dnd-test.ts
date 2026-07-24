// TEMPORARY, TEST-ONLY plugin — added solely to reproduce a real-browser drag-and-drop bug in
// TemplateInput (apps/frontend/src/pages/Upload.tsx) via a real Playwright e2e test that needs a
// real `pending_filename_conflict` download-queue item, without depending on a real external
// download plugin's real network target. `execDownload` doesn't fetch anything itself — it just
// returns a `downloads` URL pointing at a local static-file server the e2e test itself starts, and
// a fixed `filename_hint` so two different source URLs both resolve to the same on-disk filename,
// deterministically producing a real filename collision.
//
// DELETE THIS FILE once the drag-and-drop bug is confirmed fixed — it must never ship.
export function pluginInfo() {
  return {
    namespace: "e2edndtest",
    type: "download" as const,
    parameters: [],
    declared_permissions: { net: [], read: false, write: false },
    name: "E2E DnD Test Plugin (temporary)",
    author: "test",
    description: "Test-only plugin for reproducing a real drag-and-drop bug via Playwright.",
    version: "0.1",
    url_pattern: "e2e-dnd-test",
  };
}

export async function execDownload(hostArgs: Record<string, unknown>) {
  const url = hostArgs["url"] as string;
  return { downloads: [{ url, filename_hint: "dnd-test.zip" }] };
}
