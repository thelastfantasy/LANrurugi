// Adapted from ~/jellyfin-suite/apps/frontend/eslint.config.mjs (memory:
// jellyfin-suite-tooling-reference) — same Rust+React19/TS/Vite stack, single frontend package
// here so no per-package `root`/glob scoping is needed.
import js from '@eslint/js'
import globals from 'globals'
import tsParser from '@typescript-eslint/parser'
import tsPlugin from '@typescript-eslint/eslint-plugin'
import reactHooks from 'eslint-plugin-react-hooks'
import simpleImportSort from 'eslint-plugin-simple-import-sort'
import stylistic from '@stylistic/eslint-plugin'

export default [
  { ignores: ['dist/**'] },
  js.configs.recommended,
  {
    // One-off Node build scripts (e.g. `generate-emoji-zh-names.mjs`) — not part of the app
    // bundle, so they get Node's own globals (`process`/`console`/`URL`/CommonJS `require` via
    // `createRequire`) instead of `src/**`'s browser ones below, matching the environment they
    // actually run in (`node scripts/foo.mjs`, never a browser).
    files: ['scripts/**/*.mjs'],
    languageOptions: {
      globals: { ...globals.node },
    },
  },
  {
    files: ['src/**/*.{ts,tsx}'],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        projectService: true,
        tsconfigRootDir: import.meta.dirname,
        ecmaFeatures: { jsx: true },
      },
      globals: {
        ...globals.browser,
        ...globals.es2020,
      },
    },
    plugins: {
      '@typescript-eslint': tsPlugin,
      'react-hooks': reactHooks,
      'simple-import-sort': simpleImportSort,
      '@stylistic': stylistic,
    },
    rules: {
      ...tsPlugin.configs.recommended.rules,
      ...reactHooks.configs.recommended.rules,

      'no-undef': 'off',
      'no-console': ['warn', { allow: ['warn', 'error'] }],

      '@typescript-eslint/no-explicit-any': 'warn',
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_', varsIgnorePattern: '^_' }],
      '@typescript-eslint/no-non-null-assertion': 'warn',
      '@typescript-eslint/consistent-type-imports': ['warn', { prefer: 'type-imports', fixStyle: 'separate-type-imports' }],

      'padding-line-between-statements': ['warn',
        { blankLine: 'always', prev: 'import', next: '*' },
        { blankLine: 'any',     prev: 'import', next: 'import' },
        { blankLine: 'always', prev: '*', next: 'export' },
      ],
      'simple-import-sort/imports': 'warn',

      '@stylistic/quotes': ['warn', 'double', { avoidEscape: true }],

      'react-hooks/rules-of-hooks':  'error',
      'react-hooks/exhaustive-deps': 'warn',
      // react-hooks/refs is new in plugin 7.x and misfires on this project's deliberate use of
      // React's official render-time-memoization pattern (a ref written during render under a
      // `ref.current === null` guard — see Reader.tsx's `infiniteScrollResumePageRef`, whose
      // comment cites react.dev's "adjusting state directly during rendering"). The rule's
      // violation set shifts non-locally with unrelated edits (adding a fetch in an unrelated
      // function suddenly flagged 21-30 pre-existing reads), so per-line disables aren't
      // maintainable — the rule is disabled project-wide until it grows a real
      // pattern-recognition story.
      'react-hooks/refs': 'off',
    },
  },
  {
    // Barrel index.ts files — only re-exports, no expressions to flag
    files: ['src/**/index.ts'],
    rules: {
      '@typescript-eslint/no-unused-expressions': 'off',
    },
  },
  {
    // Tests — different globals (vitest for unit, node for e2e), relaxed rules
    files: ['tests/**/*.{ts,tsx}'],
    languageOptions: {
      parser: tsParser,
      parserOptions: {
        project: './tsconfig.test.json',
        tsconfigRootDir: import.meta.dirname,
      },
      globals: {
        ...globals.browser,
        ...globals.es2020,
        ...globals.node,
        ...globals.vitest,
      },
    },
    plugins: {
      '@typescript-eslint': tsPlugin,
      'simple-import-sort': simpleImportSort,
      '@stylistic': stylistic,
    },
    rules: {
      ...tsPlugin.configs.recommended.rules,
      'no-undef': 'off',
      '@typescript-eslint/no-explicit-any': 'off',
      '@typescript-eslint/no-unused-vars': ['error', { argsIgnorePattern: '^_' }],
      'simple-import-sort/imports': 'warn',
      '@stylistic/quotes': ['warn', 'double', { avoidEscape: true }],
      'padding-line-between-statements': ['warn',
        { blankLine: 'always', prev: 'import', next: '*' },
        { blankLine: 'any',     prev: 'import', next: 'import' },
      ],
    },
  },
]
