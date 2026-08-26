import { javascript } from "@codemirror/lang-javascript"
import CodeMirror from "@uiw/react-codemirror"

/** T033 (US4) — thin CodeMirror 6 wrapper for manually editing a draft revision's code. Controlled:
 * the caller owns the actual `DraftRevision` state, this component only ever reports `onChange`. */
export function CodeEditor({
  code,
  onChange,
  readOnly,
}: {
  code: string
  onChange: (code: string) => void
  readOnly?: boolean
}) {
  return (
    <div style={{ textAlign: "left" }}>
      <CodeMirror
        value={code}
        height="360px"
        extensions={[javascript({ typescript: true })]}
        readOnly={readOnly}
        onChange={onChange}
      />
    </div>
  )
}
