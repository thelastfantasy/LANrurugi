import type { IncomingMessage, ServerResponse } from "node:http"
import http from "node:http"

/** A minimal local stand-in for DeepSeek's `/chat/completions` endpoint — pointed at via
 * `LANRURUGI_DEEPSEEK_BASE_URL` (set on the worker process's own env before `fixtures.ts`'s
 * backend spawn, which inherits `process.env` by default) so the wizard's E2E coverage
 * (T047/T048) never needs a live LLM key or hits the real API (`plan.md`'s Testing note).
 *
 * `lanrurugi-llm` has TWO real call shapes that both land here, and the response framing must
 * match whichever the caller actually used:
 * - `generate.rs`'s `/plugin-wizard/generate/start` always goes through `tool_chat_streaming`
 *   (`"stream": true` in the request body) — its response must be a real SSE byte stream
 *   (`data: {...}\n\n`, terminated by `data: [DONE]\n\n`).
 * - `trial_run.rs`'s `classify_login_relevance` and `analyze_login.rs`'s own analysis call go
 *   through the older non-streaming `chat`/`json_chat` (`post_chat_completion`), which does a
 *   plain `resp.text()` + `serde_json::from_str` on the whole body — handing *that* caller an SSE
 *   stream instead of a flat JSON object fails to parse (silently downgraded to `relevant: false`
 *   by `classify_login_relevance`'s own `Err` branch, not a visible error), which is why "This
 *   failure might be login-related" never appeared in `plugin-wizard-login-detection.spec.ts`
 *   despite the queued classification response being consumed. This server distinguishes the two
 *   by checking the request body's own `"stream"` field rather than assuming one shape for every
 *   caller (confirmed live, 2026-08-26, after fixing the streaming case alone left this second,
 *   independent bug in the same file).
 *
 * Behavior is driven by a queue of canned responses, `enqueue()`d by each test before triggering
 * the wizard action that will consume one — first-in-first-out per call, so a test that expects
 * two DeepSeek calls (a generate + an auto-fix, say) enqueues two responses in the order it
 * expects them consumed. `respondToolCalls`/`respondContent` build the two response shapes
 * either caller's own parser distinguishes between; each is still a single logical response, sent
 * as a single content-only or tool_calls-only turn regardless of framing. */
export class MockLlmServer {
  private server: http.Server
  private queue: object[] = []
  private receivedRequestBodies: string[] = []

  constructor() {
    this.server = http.createServer((req, res) => this.handle(req, res))
  }

  async listen(port: number): Promise<void> {
    await new Promise<void>((resolve) => this.server.listen(port, "127.0.0.1", resolve))
  }

  async close(): Promise<void> {
    await new Promise<void>((resolve) => this.server.close(() => resolve()))
  }

  /** Queues a response body for the next call this server receives. */
  enqueue(body: object): void {
    this.queue.push(body)
  }

  /** Every raw request body this server has received so far, in order — T044's security check
   * (test credentials must never appear in any outbound LLM request) reads this directly. */
  requestBodies(): string[] {
    return this.receivedRequestBodies
  }

  private handle(req: IncomingMessage, res: ServerResponse) {
    const chunks: Buffer[] = []
    req.on("data", (c: Buffer) => chunks.push(c))
    req.on("end", () => {
      const rawBody = Buffer.concat(chunks).toString("utf8")
      this.receivedRequestBodies.push(rawBody)
      const isStreaming = (JSON.parse(rawBody || "{}") as { stream?: boolean }).stream === true
      const next = (this.queue.shift() ?? respondContent("")) as {
        choices: [{ message: { content: string | null; tool_calls?: unknown[] } }]
      }
      const message = next.choices[0].message

      if (!isStreaming) {
        // `chat`/`json_chat` (`post_chat_completion`) — a plain, non-streaming JSON body, read
        // whole via `resp.text()` then `serde_json::from_str` — the exact same shape `next` itself
        // already is, no SSE framing at all.
        res.writeHead(200, { "Content-Type": "application/json" })
        res.end(JSON.stringify(next))
        return
      }

      res.writeHead(200, {
        "Content-Type": "text/event-stream",
        "Cache-Control": "no-cache",
        Connection: "keep-alive",
      })
      if (message.tool_calls) {
        // One delta frame carrying the whole tool_calls array at index 0 — the streaming parser
        // (`tool_chat_streaming_once`) accumulates `delta.tool_calls[].function.arguments` by
        // string-concatenation per index, so a single complete frame (rather than splitting the
        // arguments JSON across multiple deltas, real DeepSeek's own token-by-token behavior)
        // still produces the correct final `ToolCall` — this mock only needs to be
        // protocol-correct, not byte-for-byte identical to a real token stream.
        const delta = { choices: [{ delta: { tool_calls: message.tool_calls }, finish_reason: null }] }
        res.write(`data: ${JSON.stringify(delta)}\n\n`)
        res.write(`data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: "tool_calls" }] })}\n\n`)
      } else {
        const delta = { choices: [{ delta: { content: message.content ?? "" }, finish_reason: null }] }
        res.write(`data: ${JSON.stringify(delta)}\n\n`)
        res.write(`data: ${JSON.stringify({ choices: [{ delta: {}, finish_reason: "stop" }] })}\n\n`)
      }
      res.write("data: [DONE]\n\n")
      res.end()
    })
  }
}

export function respondContent(content: string): object {
  return { choices: [{ message: { role: "assistant", content } }] }
}

export function respondToolCall(toolName: string, args: object): object {
  return {
    choices: [
      {
        message: {
          role: "assistant",
          content: null,
          tool_calls: [
            {
              id: "call_1",
              type: "function",
              function: { name: toolName, arguments: JSON.stringify(args) },
            },
          ],
        },
      },
    ],
  }
}
