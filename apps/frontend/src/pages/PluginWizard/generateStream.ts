import { sendJson } from "@/api/client"

/** Incremental progress `streamGenerate` reports as a generation runs — see `crates/lanrurugi-api/
 * src/plugin_wizard/generate.rs`'s own module docs for the exact event sequence this mirrors.
 * `content_delta` fires only for the final content-only round (never a tool-calling round), the
 * instant each chunk of the model's own streamed answer arrives — added specifically because that
 * final round alone was observed taking 80+ seconds with zero visibility into whether anything was
 * happening (2026-08-24). */
export type GenerateProgressEvent =
  | { kind: "fetch_page"; url: string }
  | { kind: "fetch_result"; url: string; status: string }
  | { kind: "content_delta"; text: string }

/** Mirrors `readErrorBody`'s `{error, detail, raw_output}` shape (`api/client.ts`) — the SSE
 * `error` event carries the identical fields the old plain-JSON `/plugin-wizard/generate` response
 * used to, so every existing `err instanceof ApiError && err.status === 422` check just becomes
 * `err instanceof GenerateStreamError && err.code === "ai_output_not_code"`. */
export class GenerateStreamError extends Error {
  code: string
  detail?: string
  rawOutput?: string

  constructor(code: string, detail?: string, rawOutput?: string) {
    // `detail` first, `rawOutput` as a fallback — mirrors `readErrorBody`'s own `detail ?? raw_
    // output` exactly (`api/client.ts`). The backend never fills both for the same error (`ai_
    // output_not_code` only ever sends `raw_output`, `llm_unavailable` only ever sends `detail`),
    // but without this fallback the `ai_output_not_code` case's `message` was just the bare code
    // string "ai_output_not_code" with zero diagnostic content — a real, observed regression from
    // this class's introduction (the non-streaming predecessor's `readErrorBody` already handled
    // this correctly; this rewrite dropped the fallback when porting to SSE).
    const extra = detail ?? rawOutput
    super(extra ? `${code}: ${extra}` : code)
    this.code = code
    this.detail = detail
    this.rawOutput = rawOutput
  }
}

/** Drives `POST /plugin-wizard/generate/start` + `GET /plugin-wizard/generate/stream/{id}` (the
 * two-step SSE pattern `EventSource` being GET-only forces — see `generate.rs`'s own module docs)
 * via a plain `EventSource`, same client `useCompareStream.ts` already established for this
 * codebase's other real-time endpoint. Every one of `GenerationStep.tsx`/`TrialRunResult.tsx`'s
 * auto-fix/`RefinePanel.tsx`'s refine calls this directly rather than each hand-rolling its own
 * `EventSource` wiring — all three want the identical event sequence, just with a different
 * request body. */
export function streamGenerate(
  body: Record<string, unknown>,
  onProgress: (event: GenerateProgressEvent) => void,
): Promise<{ code: string; explanation: string; resolvedCredentialValues: Record<string, string> }> {
  return sendJson<{ generation_id: string }>("POST", "/plugin-wizard/generate/start", body).then(
    ({ generation_id }) =>
      new Promise((resolve, reject) => {
        // Guards against `source.onerror`'s generic connection-level callback double-firing after
        // `done`/a named `error` event already settled this promise — `EventSource` can't tell
        // "server closed the stream normally" apart from "connection actually dropped", so
        // `onerror` fires on ordinary stream-end too (identical reasoning to `useCompareStream.ts`'s
        // own `finishedRef`, just a plain closure variable here since this isn't a React hook).
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
        // A real, distinguishable server-side failure is its own named `event: error` SSE message
        // carrying `{error, detail?, raw_output?}` as `data` — this is NOT the same thing as
        // `source.onerror` below (the browser's generic connection-level callback, which never
        // carries `data` at all), despite both being reached under the name "error".
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
