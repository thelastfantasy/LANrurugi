import { Highlight, themes } from "prism-react-renderer"

import { FONT_SIZE_SM } from "@/theme"

/** Syntax-highlighted code block for the job detail view. Uses `prism-react-renderer`, chosen
 * over `react-syntax-highlighter` (bundle size) and `shiki` (async/WASM, overkill here). */
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
            fontSize: FONT_SIZE_SM,
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
