import { defineConfig, mergeConfig } from 'vitest/config'

import viteConfig from './vite.config'

// Layer 1 (spec User Story 1 Acceptance Scenario 3): fast, no-backend unit coverage over vite's
// own existing config — Vitest 4.1's native Vite 8 support means no separate Tailwind config is
// needed here (research.md §1).
export default mergeConfig(
  viteConfig,
  defineConfig({
    test: {
      environment: 'jsdom',
      globals: true,
      include: ['tests/unit/**/*.test.{ts,tsx}'],
      setupFiles: ['./tests/unit/setup.ts'],
    },
  }),
)
