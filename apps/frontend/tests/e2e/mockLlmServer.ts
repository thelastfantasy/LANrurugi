import type { IncomingMessage, ServerResponse } from "node:http"
import http from "node:http"

/** A minimal local stand-in for DeepSeek's `/chat/completions` endpoint — `lanrurugi-llm`'s
 * `post_chat_completion` is pointed at this via `LANRURUGI_DEEPSEEK_BASE_URL` (set on the worker
 * process's own env before `fixtures.ts`'s backend spawn, which inherits `process.env` by
 * default) so the wizard's E2E coverage (T047/T048) never needs a live LLM key or hits the real
 * API (`plan.md`'s Testing note).
 *
 * Behavior is driven by a queue of canned responses, `enqueue()`d by each test before triggering
 * the wizard action that will consume one — first-in-first-out per call, so a test that expects
 * two DeepSeek calls (a generate + an auto-fix, say) enqueues two responses in the order it
 * expects them consumed. `respondToolCalls`/`respondContent` build the two response shapes
 * `tool_chat`'s own parser (`lanrurugi-llm::parse_tool_chat_response`) distinguishes between. */
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
      const next = this.queue.shift()
      res.writeHead(200, { "Content-Type": "application/json" })
      res.end(JSON.stringify(next ?? respondContent("")))
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
