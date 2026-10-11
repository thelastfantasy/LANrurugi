import path from "node:path"
import { fileURLToPath } from "node:url"

const __dirname = path.dirname(fileURLToPath(import.meta.url))

// Single point of truth for resolving `test-fixtures/archives/*` (data-model.md's Scenario
// Contract) — e2e-only. The unit layer never needs this: per spec User Story 1 Acceptance
// Scenario 3, unit-level scenarios are pure logic with no I/O.
export function fixturePath(name: string): string {
  return path.resolve(__dirname, "../../../../test-fixtures/archives", name)
}
