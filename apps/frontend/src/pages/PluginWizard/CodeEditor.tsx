import { javascript } from "@codemirror/lang-javascript"
import CodeMirror from "@uiw/react-codemirror"

/** Controlled CodeMirror wrapper: the caller owns the code state, this only reports `onChange`. */
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
