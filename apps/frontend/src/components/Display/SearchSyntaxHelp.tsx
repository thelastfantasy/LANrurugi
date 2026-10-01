import type { ReactNode } from "react"
import { useTranslation } from "react-i18next"

import { FONT_SIZE_SM } from "@/theme"

/** Inline `<code>`-styled span for a literal search-syntax example (`pages:>100`, `-tag`, …). */
function SyntaxExample({ children }: { children: ReactNode }) {
  return (
    <code
      style={{
        fontFamily: "monospace",
        fontSize: FONT_SIZE_SM,
        background: "rgba(128, 128, 128, 0.18)",
        borderRadius: 3,
        padding: "1px 5px",
      }}
    >
      {children}
    </code>
  )
}

/** Full search-syntax reference, shared wherever this app claims "same syntax as the main search
 * bar" (e.g. the new-category dialog). */
export function SearchSyntaxHelp() {
  const { t } = useTranslation()
  return (
    <div>
      <div style={{ fontWeight: 700, marginBottom: 8 }}>{t("library.searchSyntaxHelp")}</div>
      {/* `lrr.css` sets `li { list-style: none }`, so each `<li>` re-asserts `disc` itself. */}
      <ul style={{ margin: 0, paddingLeft: 18 }}>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>{t("library.searchSyntaxAndSeparator")}</li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>
          {t("library.searchSyntaxNegation")} <SyntaxExample>-tag</SyntaxExample>
        </li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>
          {t("library.searchSyntaxExactMatch")} <SyntaxExample>&quot;tag&quot;</SyntaxExample> / <SyntaxExample>tag$</SyntaxExample>
        </li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>{t("library.searchSyntaxSpaceNeedsQuotes")}</li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>
          {t("library.searchSyntaxQuoteExactCombo")} <SyntaxExample>&quot;tag with spaces&quot;$</SyntaxExample>
        </li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>
          {t("library.searchSyntaxMultiTagExample")}{" "}
          <SyntaxExample>
            female:&quot;huge breasts&quot; female:milf
          </SyntaxExample>
        </li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>
          {t("library.searchSyntaxEscapedQuote")}{" "}
          <SyntaxExample>artist:&quot;foo\&quot;bar&quot;</SyntaxExample>
        </li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>{t("library.searchSyntaxWildcard")}</li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>
          {t("library.searchSyntaxDateAdded")} <SyntaxExample>date_added:2026-08-20</SyntaxExample>
        </li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>
          {t("library.searchSyntaxNumericCompare")} <SyntaxExample>pages:&gt;100</SyntaxExample>, <SyntaxExample>read:&lt;5</SyntaxExample>
        </li>
        <li style={{ listStyleType: "disc" }}>
          {t("library.searchSyntaxRating")} <SyntaxExample>rating:&gt;=4</SyntaxExample>, <SyntaxExample>rating:&lt;3</SyntaxExample>, <SyntaxExample>rating:=5</SyntaxExample>
        </li>
      </ul>
    </div>
  )
}
