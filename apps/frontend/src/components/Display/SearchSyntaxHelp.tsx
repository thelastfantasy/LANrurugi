import type { ReactNode } from "react"
import { useTranslation } from "react-i18next"

import { FONT_SIZE_SM } from "@/theme"

/** Inline `<code>`-styled span for a literal search-syntax example (`pages:>100`, `-tag`, …)
 * inside the syntax-help popover — plain JSX children, not `dangerouslySetInnerHTML`: these
 * examples are static string/JSX literals this component itself writes, never user- or
 * translation-supplied markup, so there's no HTML string to parse/inject in the first place. */
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

/** The full search-syntax reference content — originally `SearchBar.tsx`'s own inline
 * `ClickPopover` label, factored out so it can be reused verbatim wherever else this app claims
 * "same syntax as the main search bar" (e.g. the new-category dialog's and `Categories.tsx`'s own
 * dynamic-category predicate help, both of which used to just say that in plain text instead of
 * actually showing the same rich reference — confirmed live, 2026-08-27, that the plain-text
 * version read as noticeably less helpful side by side with this one). */
export function SearchSyntaxHelp() {
  const { t } = useTranslation()
  return (
    <div>
      <div style={{ fontWeight: 700, marginBottom: 8 }}>{t("library.searchSyntaxHelp")}</div>
      {/* `lrr.css` (legacy, not editable — see CLAUDE.md) carries a bare `li { list-style:
          none }` tag-selector rule, which wins against `list-style-type: disc` set only on
          the parent `<ul>` — `list-style-type` is inherited, but a plain, non-inherited
          declaration on the element itself (this `li` rule) beats an inherited value in the
          cascade regardless of where the inherited value came from, so each `<li>` below
          re-asserts `disc` directly on itself rather than relying on inheriting it from the
          `<ul>` (confirmed live: the `<ul>`-only version silently rendered with zero
          bullets). Also plain block-flow, not `display: flex` — a flex container never
          renders its children's `::marker` at all (the browser's own list-marker box only
          paints in normal block/list-item flow), which was this component's very first,
          independent reason the bullets disappeared before the `lrr.css` rule above was
          even found. `<li>`'s own `marginBottom` reproduces the row-spacing a `gap` would
          have given a flex layout, without that flex/marker conflict. */}
      <ul style={{ margin: 0, paddingLeft: 18 }}>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>{t("library.searchSyntaxAndSeparator")}</li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>
          {t("library.searchSyntaxNegation")} <SyntaxExample>-tag</SyntaxExample>
        </li>
        <li style={{ listStyleType: "disc", marginBottom: 6 }}>
          {t("library.searchSyntaxExactMatch")} <SyntaxExample>&quot;tag&quot;</SyntaxExample> / <SyntaxExample>tag$</SyntaxExample>
        </li>
        {/* Legacy's own tokenizer only ever splits on comma (a bare space just gets
            skipped, not treated as a delimiter), so `tag with spaces$` was a real, useful
            legacy idiom — a whole space-containing value, quote-free, made exact by a
            trailing `$`. This app's own tokenizer (`grammar.rs`, issue #59) made space a
            real delimiter too, which breaks that idiom: `tag with spaces$` now parses as
            three independent AND'd tokens (`tag`, `with`, `spaces$`), not one. Worth its
            own explicit line rather than leaving it to be inferred from the exact-match
            line above, since a user coming from legacy specifically reaching for that bare
            `$` idiom would otherwise silently get three loosely-ANDed fuzzy tokens instead
            of the one exact tag they meant. */}
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
