import { Highlight, themes } from "prism-react-renderer"

import { FONT_SIZE_10PT } from "../theme"

/** Syntax-highlighted code block for the job detail view (finished jobs' JSON `result`).
 *
 * Uses `prism-react-renderer` (Prism) — the closest thing to a "source-mapped code highlighter"
 * for React: tokenizes a string and renders colored spans with a theme. `json` is one of its
 * bundled grammars, no extra setup needed. Chosen over `react-syntax-highlighter` (heavier —
 * would bloat the bundle past the existing >500 kB chunk warning) and `shiki` (async/WASM,
 * overkill for tiny job-result payloads). */
export function CodeBlock({
  code,
  language = "json",
}: {
  code: string
  language?: string
}) {
  return (
    <Highlight theme={themes.github} code={code} language={language}>
      {({ className, style, tokens, getLineProps, getTokenProps }) => (
        <pre
          className={className}
          style={{
            ...style,
            margin: 0,
            padding: "8px 12px",
            fontSize: FONT_SIZE_10PT,
            lineHeight: 1.4,
            overflow: "auto",
            borderRadius: 4,
            maxWidth: "100%",
          }}
        >
          {tokens.map((line, i) => (
            <div key={i} {...getLineProps({ line })}>
              {line.map((token, key) => (
                <span key={key} {...getTokenProps({ token })} />
              ))}
            </div>
          ))}
        </pre>
      )}
    </Highlight>
  )
}
