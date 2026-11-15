/** A keyboard key rendered as a physical keycap; the look lives in `.reader-keycap` (index.css),
 * so the reader's OSDs and the library's shortcut sheet share one language. */
export function Keycap({ children }: { children: React.ReactNode }) {
  return (
    <code className="reader-keycap">
      <span className="reader-keycap-face">{children}</span>
    </code>
  );
}
