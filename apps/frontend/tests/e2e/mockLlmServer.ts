import type { IncomingMessage, ServerResponse } from "node:http"
import http from "node:http"

/** A minimal local stand-in for DeepSeek's `/chat/completions` endpoint — `lanrurugi-llm`'s
 * `tool_chat_streaming` (`crates/lanrurugi-llm/src/lib.rs`) is pointed at this via
 * `LANRURUGI_DEEPSEEK_BASE_URL` (set on the worker process's own env before `fixtures.ts`'s
 * backend spawn, which inherits `process.env` by default) so the wizard's E2E coverage
 * (T047/T048) never needs a live LLM key or hits the real API (`plan.md`'s Testing note).
 *
 * Every real call this server receives is a streaming one (`"stream": true` in the request body
 * — `generate.rs`'s own `/plugin-wizard/generate/start` always goes through `tool_chat_streaming`,
 * never the older non-streaming `tool_chat`), so responses are always framed as a real SSE byte
 * stream (`data: {...}\n\n`, terminated by `data: [DONE]\n\n`), not a single plain JSON body — a
 * plain-JSON response here previously left the streaming parser waiting on SSE frames that never
 * arrived until the caller's own timeout, with the request itself appearing to "hang" rather than
 * fail with any diagnosable error (confirmed live, 2026-08-26, tracing the cause of `plugin-
 * wizard.spec.ts`'s "Trial run" button never appearing after "Generate" was clicked).
 *
 * Behavior is driven by a queue of canned responses, `enqueue()`d by each test before triggering
 * the wizard action that will consume one — first-in-first-out per call, so a test that expects
 * two DeepSeek calls (a generate + an auto-fix, say) enqueues two responses in the order it
 * expects them consumed. `respondToolCalls`/`respondContent` build the two response shapes
 * `tool_chat_streaming`'s own parser distinguishes between; each is still a single logical
 * response, just replayed as one SSE delta frame internally rather than sent as a plain body. */
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
      this.receivedRequestBodies.push(Buffer.concat(chunks).toString("utf8"))
      const next = (this.queue.shift() ?? respondContent("")) as {
        choices: [{ message: { content: string | null; tool_calls?: unknown[] } }]
      }
      const message = next.choices[0].message
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
