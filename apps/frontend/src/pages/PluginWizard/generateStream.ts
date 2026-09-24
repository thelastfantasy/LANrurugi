import { sendJson } from "@/api/client"

/** Incremental progress `streamGenerate` reports as a generation runs.
 * `content_delta` fires only for the final content-only round, as each chunk of the model's streamed answer arrives. */
export type GenerateProgressEvent =
  | { kind: "fetch_page"; url: string }
  | { kind: "fetch_result"; url: string; status: string }
  | { kind: "content_delta"; text: string }

/** Mirrors `readErrorBody`'s `{error, detail, raw_output}` shape (`api/client.ts`). */
export class GenerateStreamError extends Error {
  code: string
  detail?: string
  rawOutput?: string

  constructor(code: string, detail?: string, rawOutput?: string) {
    // Backend fills at most one of `detail`/`rawOutput` per error code; without a fallback
    // `message` would be just the bare code string for whichever field is empty.
    const extra = detail ?? rawOutput
    super(extra ? `${code}: ${extra}` : code)
    this.code = code
    this.detail = detail
    this.rawOutput = rawOutput
  }
}

/** Drives `POST /plugin-wizard/generate/start` + `GET /plugin-wizard/generate/stream/{id}` via a
 * plain `EventSource` — shared by generate, auto-fix, and refine calls. */
export function streamGenerate(
  body: Record<string, unknown>,
  onProgress: (event: GenerateProgressEvent) => void,
): Promise<{ code: string; explanation: string; resolvedCredentialValues: Record<string, string> }> {
  return sendJson<{ generation_id: string }>("POST", "/plugin-wizard/generate/start", body).then(
    ({ generation_id }) =>
      new Promise((resolve, reject) => {
        // `source.onerror` also fires on ordinary stream-end (EventSource can't tell that apart
        // from a real drop), so this guards against it double-firing after `done`/`error` settled.
        let finished = false
        const source = new EventSource(
          `/api/plugin-wizard/generate/stream/${encodeURIComponent(generation_id)}`,
        )

        function finish(action: () => void) {
          finished = true
          source.close()
          action()
        }

        source.addEventListener("fetch_page", (ev) => {
          const data = JSON.parse((ev as MessageEvent<string>).data) as { url: string }
          onProgress({ kind: "fetch_page", url: data.url })
        })
        source.addEventListener("fetch_result", (ev) => {
          const data = JSON.parse((ev as MessageEvent<string>).data) as { url: string; status: string }
          onProgress({ kind: "fetch_result", url: data.url, status: data.status })
        })
        source.addEventListener("content_delta", (ev) => {
          const data = JSON.parse((ev as MessageEvent<string>).data) as { text: string }
          onProgress({ kind: "content_delta", text: data.text })
        })
        source.addEventListener("done", (ev) => {
          const data = JSON.parse((ev as MessageEvent<string>).data) as {
            code: string
            explanation: string
            resolved_credential_values?: Record<string, string>
          }
          finish(() =>
            resolve({
              code: data.code,
              explanation: data.explanation,
              resolvedCredentialValues: data.resolved_credential_values ?? {},
            }),
          )
        })
        // Named `error` event (carries `data`) is a real server-side failure, distinct from
        // `source.onerror` below (generic connection callback, never carries `data`).
        source.addEventListener("error", (ev) => {
          const messageEvent = ev as MessageEvent<string> | undefined
          if (!messageEvent?.data) return
          const data = JSON.parse(messageEvent.data) as {
            error: string
            detail?: string
            raw_output?: string
          }
          finish(() => reject(new GenerateStreamError(data.error, data.detail, data.raw_output)))
        })
        source.onerror = () => {
          if (!finished) {
            finish(() =>
              reject(new GenerateStreamError("connection_lost", "Connection to the generation stream was lost.")),
            )
          }
        }
      }),
  )
}
