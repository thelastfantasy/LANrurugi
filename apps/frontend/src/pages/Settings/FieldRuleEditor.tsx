import { Fragment, useState } from "react"
import { useTranslation } from "react-i18next"

import { useAiCondition, useLlmKeyStatus, useParseCondition } from "@/api/hooks"
import type { Condition, FieldOperator, RuleValue } from "@/api/types"
import { Modal, Tooltip } from "@/components/common-ui/Display"
import { NullableNumberInput } from "@/components/common-ui/Form/NumberInput"
import { CodeBlock } from "@/components/Display"
import { TagInput } from "@/components/Form"
import { toast } from "@/toast"

import { ICON_BUTTON_STYLE } from "../Upload/shared"


/** What kind of value a candidate field holds, which fixes its operators and its control.
 *
 * Derived from the field name rather than sent by the source: the names come from the SDK's own
 * candidate interface, where the type is already fixed at compile time, so a second declaration over
 * the wire would only be one more thing that can disagree with it. */
type FieldKind = "number" | "date" | "text" | "list"

/** Tag namespaces offered as fields of their own, mirroring the host's own list.
 *
 * A namespace is a list field: "language" holds every `language:` value a work carries. Offering them
 * separately is what makes "in Chinese or Japanese, or carrying no language at all" sayable — over
 * the raw tag list the last part cannot be written, since excluding specific tags cannot say "none of
 * this kind". */
export const TAG_NAMESPACES = [
  "language",
  "artist",
  "group",
  "parody",
  "character",
  "female",
  "male",
  "other",
] as const

const FIELD_KINDS: Record<string, FieldKind> = {
  posted_at: "date",
  rating: "number",
  pages: "number",
  title: "text",
  category: "text",
  uploader: "text",
  tags: "list",
  ...Object.fromEntries(TAG_NAMESPACES.map((ns) => [ns, "list" as const])),
}

const OPERATORS: Record<FieldKind, FieldOperator[]> = {
  // `older_than` first: it is the one that makes "wait until it has settled" expressible, which is the
  // reason date filtering exists here at all.
  date: ["older_than", "newer_than"],
  number: ["gte", "lte", "eq"],
  text: ["equals", "contains", "in", "not_in"],
  // `is_empty` last: it answers a different question from the two above, and is the only way to say
  // "carries nothing of this kind".
  list: ["includes_all", "includes_none", "is_empty", "is_not_empty"],
}

/** Durations offered for a date rule. Relative, never an instant: a subscription is a standing
 *  instruction, and a fixed date would mean something different on every run. */
const DURATIONS = [
  { secs: 3600, key: "subscriptions.oneHour" },
  { secs: 3 * 3600, key: "subscriptions.threeHours" },
  { secs: 12 * 3600, key: "subscriptions.twelveHours" },
  { secs: 86400, key: "subscriptions.oneDay" },
  { secs: 7 * 86400, key: "subscriptions.oneWeek" },
  { secs: 30 * 86400, key: "subscriptions.oneMonth" },
]

export function kindOf(field: string): FieldKind {
  return FIELD_KINDS[field] ?? "text"
}

function defaultValueFor(kind: FieldKind): RuleValue {
  switch (kind) {
    case "date":
      return 3 * 3600
    case "number":
      return 0
    case "list":
      return []
    default:
      return ""
  }
}

function defaultOperatorFor(kind: FieldKind): FieldOperator {
  return OPERATORS[kind][0]
}

/** `is_empty`/`is_not_empty` compare nothing, so the stored value is a placeholder. */
function operatorNeedsValue(operator: FieldOperator): boolean {
  return operator !== "is_empty" && operator !== "is_not_empty"
}

function defaultRule(fields: string[]): Condition {
  const field = fields[0] ?? "title"
  const kind = kindOf(field)
  return { kind: "rule", field, operator: defaultOperatorFor(kind), value: defaultValueFor(kind) }
}

/** One row: a single comparison. */
function RuleRow({
  fields,
  rule,
  tagSuggestions,
  onChange,
}: {
  fields: string[]
  rule: Extract<Condition, { kind: "rule" }>
  /** Passed down rather than subscribed to per row: a tree with ten rules would otherwise mount ten
   *  copies of the same query. */
  tagSuggestions: string[]
  onChange: (next: Condition) => void
}) {
  // The numeric value field's own draft: blank is a legitimate mid-edit state, and committing a
  // blank still writes 0 (what this field always did) — `NumberInput` alone would fight the typist
  // by snapping an emptied box back to a number on the next keystroke.
  const [numberDraft, setNumberDraft] = useState<number | "">(
    typeof rule.value === "number" ? rule.value : "",
  )
  const { t } = useTranslation()
  const kind = kindOf(rule.field)
  const wantsList = kind === "list" || rule.operator === "in" || rule.operator === "not_in"
  /** The raw text while a list value is being typed; `null` once focus leaves and the normalised
   *  list takes over again. */
  const [draft, setDraft] = useState<string | null>(null)

  return (
    // A grid, not a flex row: `.stdinput` carries its own `width: 80%` from the legacy theme, so three
    // of them in a flex row each claimed 80% and wrapped onto separate lines.
    <div className="rule-row">
      <select
        className="stdinput"
        value={rule.field}
        onChange={(e) => {
          const f = e.target.value
          const k = kindOf(f)
          // Operator and value reset with the field: keeping `contains` after switching to a date
          // would leave a rule the host can only reject.
          onChange({ kind: "rule", field: f, operator: defaultOperatorFor(k), value: defaultValueFor(k) })
        }}
      >
        {fields.map((f) => (
          <option key={f} value={f}>
            {t(`subscriptions.field.${f}`, { defaultValue: f })}
          </option>
        ))}
      </select>

      <select
        className="stdinput"
        value={rule.operator}
        onChange={(e) => onChange({ ...rule, operator: e.target.value as FieldOperator })}
      >
        {OPERATORS[kind].map((op) => (
          <option key={op} value={op}>
            {t(`subscriptions.operator.${op}`)}
          </option>
        ))}
      </select>

      {!operatorNeedsValue(rule.operator) ? (
        // Nothing to compare against — the operator is the whole condition. A disabled box would
        // only invite someone to type into it.
        <span />
      ) : kind === "date" ? (
        <select
          className="stdinput"
          value={String(rule.value)}
          onChange={(e) => onChange({ ...rule, value: Number(e.target.value) })}
        >
          {DURATIONS.map((d) => (
            <option key={d.secs} value={d.secs}>
              {t(d.key)}
            </option>
          ))}
        </select>
      ) : kind === "number" ? (
        // A draft, so clearing the box mid-edit does not slam the rule's value to 0 and fight the
        // typist; committing a blank still writes 0, which is what this field always did.
        <NullableNumberInput
          className="stdinput"
          style={{ width: 90 }}
          step={0.1}
          value={numberDraft}
          onValueChange={(v) => {
            setNumberDraft(v);
            onChange({ ...rule, value: Number(v) || 0 });
          }}
        />
      ) : (
        rule.field === "tags" ? (
          // The same tag input used for archive metadata, so completion works the same way wherever
          // tags are typed. It owns its own draft text, which is also why the space-eating fix below
          // is not needed on this path.
          <TagInput
            value={Array.isArray(rule.value) ? rule.value.join(", ") : String(rule.value)}
            suggestions={tagSuggestions}
            onChange={(next) =>
              onChange({
                ...rule,
                value: next.split(",").map((x) => x.trim()).filter(Boolean),
              })
            }
          />
        ) : (
          <div className="rule-value-with-hint">
            <input
              className="stdinput"
              // Shows what was typed, not the normalised list. Splitting on every keystroke trimmed the
              // space the user had just pressed — `female:big ` became `female:big` before the next
              // letter arrived, so a tag with a space in it could not be typed at all.
              value={draft ?? (Array.isArray(rule.value) ? rule.value.join(", ") : String(rule.value))}
              placeholder={wantsList ? (t("subscriptions.commaSeparated") ?? undefined) : undefined}
              onChange={(e) => {
                const raw = e.target.value
                if (wantsList) {
                  setDraft(raw)
                  // The stored value stays a list so the rule is always well-formed, but empty
                  // fragments are kept out of it rather than out of the text being typed.
                  onChange({
                    ...rule,
                    value: raw.split(",").map((x) => x.trim()).filter(Boolean),
                  })
                } else {
                  onChange({ ...rule, value: raw })
                }
              }}
              // Once focus leaves, the text catches up with the stored list — trailing commas and
              // stray spaces disappear, which is the moment that tidying is not disruptive.
              onBlur={() => setDraft(null)}
            />
            {wantsList && (
              // A filled field hides the comma-separated placeholder, so keep the affordance visible
              // as an icon with the same explanation.
              <Tooltip
                label={t("subscriptions.commaSeparated") ?? ""}
                zIndex={9700}
                wrapperStyle={{
                  position: "absolute",
                  right: 6,
                  top: "calc(50% + 2px)",
                  height: "100%",
                  transform: "translateY(-50%)",
                  display: "inline-flex",
                  alignItems: "center",
                }}
              >
                <button
                  type="button"
                  className="rule-value-hint-button"
                  aria-label={t("subscriptions.commaSeparated") ?? undefined}
                  // Mobile tap does not always focus a button; focusing explicitly makes the
                  // tooltip's own focus handler open it there too.
                  onClick={(e) => e.currentTarget.focus()}
                >
                  <i className="fa fa-info-circle rule-value-hint-icon" aria-hidden="true" />
                </button>
              </Tooltip>
            )}
          </div>
        )
      )}
    </div>
  )
}

/** One node and, for a group, everything under it. */
function ConditionNode({
  fields,
  node,
  tagSuggestions,
  onChange,
  onRemove,
  depth,
  removeLabelKey = "subscriptions.removeCondition",
}: {
  fields: string[]
  tagSuggestions: string[]
  node: Condition
  onChange: (next: Condition) => void
  onRemove?: () => void
  depth: number
  removeLabelKey?: string
}) {
  const { t } = useTranslation()

  const removeButton = onRemove && (
    <button type="button" className="stdbtn rule-action" onClick={onRemove}>
      <i className="fa fa-times" aria-hidden="true"></i> {t(removeLabelKey)}
    </button>
  )

  if (node.kind === "rule") {
    return (
      <div className="rule-entry">
        <RuleRow
          fields={fields}
          rule={node}
          tagSuggestions={tagSuggestions}
          onChange={onChange}
        />
        {/* The two actions travel together as one block: when the rule row wraps below the dialog's
            width, "delete" must not be left alone on its own line. */}
        <div className="rule-entry-actions">
          <button
            type="button"
            className="stdbtn rule-action"
            onClick={() => onChange({ kind: "not", child: node })}
          >
            <i className="fa fa-ban" aria-hidden="true"></i> {t("subscriptions.negateCondition")}
          </button>
          {removeButton}
        </div>
      </div>
    )
  }

  if (node.kind === "not") {
    return (
      <div className="field-rule-group" style={{ marginBottom: 4 }}>
        <div className="group-header">
          <strong className="group-label">{t("subscriptions.exceptWhen")}</strong>
          <div className="rule-entry-actions">
            <button
              type="button"
              className="stdbtn rule-action"
              onClick={() => onChange(node.child)}
            >
              <i className="fa fa-undo" aria-hidden="true"></i>{" "}
              {t("subscriptions.unnegateCondition")}
            </button>
            {removeButton}
          </div>
        </div>
        <div style={{ marginLeft: 16 }}>
          <ConditionNode
            fields={fields}
            tagSuggestions={tagSuggestions}
            node={node.child}
            onChange={(child) => onChange({ kind: "not", child })}
            depth={depth + 1}
          />
          {node.child.kind === "rule" && depth < 3 && (
            // Lets a single negated rule grow into a negated group. Without it, "except when X" could
            // never become "except when X and Y" — the all/any control only exists on a group.
            <div className="rule-entry-actions" style={{ marginTop: 2 }}>
              <input
                type="button"
                className="stdbtn rule-action"
                // Named apart from the outer "add condition": both would otherwise read identically
                // while adding to different groups — one narrows the subscription, the other widens
                // what is excluded from it.
                value={t("subscriptions.addToException") ?? undefined}
                onClick={() =>
                  onChange({
                    kind: "not",
                    child: { kind: "all", children: [node.child, defaultRule(fields)] },
                  })
                }
              />
            </div>
          )}
        </div>
      </div>
    )
  }

  // A group: its children's connective is the group's own kind, which is what makes siblings
  // independently AND-able or OR-able at each level.
  const children = node.children
  return (
    <div className="field-rule-group" style={{ marginBottom: 4 }}>
      <div className="group-header">
        <span className="group-label">
          {node.kind === "all" ? t("subscriptions.matchAllLabel") : t("subscriptions.matchAnyLabel")}
        </span>
        <div className="rule-entry-actions">
          <button
            type="button"
            className="stdbtn rule-action"
            onClick={() => onChange({ kind: "not", child: node })}
          >
            <i className="fa fa-ban" aria-hidden="true"></i> {t("subscriptions.negateCondition")}
          </button>
          {removeButton}
        </div>
      </div>

      <div style={{ marginLeft: 16 }}>
        {children.map((child, i) => (
          <Fragment key={i}>
            {/* Between the conditions it joins, rather than once at the top of the group: a single
                control up there reads as a property of the group and gives no sign it decides how
                *these two* relate. */}
            {i > 0 && (
              <div className="group-connective">
                <select
                  className="stdinput"
                  value={node.kind}
                  aria-label={t("subscriptions.connective") ?? "How these combine"}
                  onChange={(e) =>
                    onChange({ kind: e.target.value as "all" | "any", children })
                  }
                >
                  <option value="all">{t("subscriptions.and")}</option>
                  <option value="any">{t("subscriptions.or")}</option>
                </select>
              </div>
            )}
            <ConditionNode
              fields={fields}
              tagSuggestions={tagSuggestions}
              node={child}
              depth={depth + 1}
              onChange={(next) =>
                onChange({ kind: node.kind, children: children.map((c, j) => (j === i ? next : c)) })
              }
              onRemove={() =>
                onChange({ kind: node.kind, children: children.filter((_, j) => j !== i) })
              }
            />
          </Fragment>
        ))}

        <div className="rule-entry-actions" style={{ marginTop: 2 }}>
          <input
            type="button"
            className="stdbtn rule-action"
            value={t("subscriptions.addCondition") ?? undefined}
            onClick={() =>
              onChange({ kind: node.kind, children: [...children, defaultRule(fields)] })
            }
          />
          {/* Nesting is capped: past a few levels a condition becomes unreadable, and the intent is
              almost always expressible more simply. */}
          {depth < 3 && (
            <input
              type="button"
              className="stdbtn rule-action"
              value={t("subscriptions.addGroup") ?? undefined}
              onClick={() =>
                onChange({
                  kind: node.kind,
                  children: [...children, { kind: "any", children: [defaultRule(fields)] }],
                })
              }
            />
          )}
        </div>

        {children.length === 0 && (
          // An empty group is unsatisfiable rather than vacuously true, so say so instead of letting
          // the user believe it matches everything.
          <p style={{ opacity: 0.8, fontSize: "0.9em", margin: "2px 0 0" }}>
            {t("subscriptions.emptyGroupMatchesNothing")}
          </p>
        )}
      </div>
    </div>
  )
}

/** Natural-language entry point for the same condition tree the manual controls edit. The model
 * returns the compact DSL; the backend parser validates it into a real `Condition`. */
function AiConditionCreator({
  fields,
  onApply,
}: {
  fields: string[]
  onApply: (condition: Condition) => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const llmStatus = useLlmKeyStatus()
  // Offered only when it can actually run: without a key the only thing this button can do is
  // explain that it cannot — and an unusable AI affordance in a filter editor invites the belief
  // that the rules were written with help that never happened.
  const available = llmStatus.data?.configured === true

  if (!open) {
    if (!available) return null
    return (
      <div className="sub-form-grid">
        <label>{t("subscriptions.aiConditionLabel")}</label>
        <div>
          <input
            type="button"
            className="stdbtn"
            value={t("subscriptions.aiCreateCondition") ?? undefined}
            onClick={() => setOpen(true)}
          />
        </div>
      </div>
    )
  }

  return (
    <AiConditionPanel
      fields={fields}
      onApply={onApply}
      onClose={() => setOpen(false)}
    />
  )
}

/** Mounted only after the user opens the AI creator: the mutation/status hooks then require the
 *  app's QueryClient only in the interaction that actually uses them. */
function AiConditionPanel({
  fields,
  onApply,
  onClose,
}: {
  fields: string[]
  onApply: (condition: Condition) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const ai = useAiCondition()
  const llmStatus = useLlmKeyStatus()
  const [prompt, setPrompt] = useState("")
  const [error, setError] = useState<string | null>(null)
  const missingKey = llmStatus.data?.configured === false

  async function generate() {
    setError(null)
    try {
      const result = await ai.mutateAsync({ prompt, fields })
      onApply(result.condition)
      setPrompt("")
      onClose()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="sub-form-grid">
      <label style={{ alignSelf: "start", paddingTop: 5 }}>
        {t("subscriptions.aiConditionLabel")}
      </label>
      <div>
        <textarea
        className="stdinput"
        rows={3}
        style={{ width: "100%", boxSizing: "border-box", resize: "vertical" }}
        value={prompt}
        placeholder={t("subscriptions.aiConditionPrompt") ?? undefined}
        onChange={(e) => setPrompt(e.target.value)}
      />
      <p className="sub-form-hint">{t("subscriptions.aiConditionHint")}</p>
      {missingKey && <p className="sub-form-hint">{t("subscriptions.aiNoKey")}</p>}
      {error && <p style={{ color: "red" }}>{error}</p>}
        <div style={{ display: "flex", gap: 6, marginTop: 4 }}>
          <input
            type="button"
            className="stdbtn"
            value={
              (ai.isPending
                ? t("subscriptions.aiGenerating")
                : t("subscriptions.aiGenerate")) ?? undefined
            }
            disabled={ai.isPending || !prompt.trim() || missingKey}
            onClick={() => void generate()}
          />
          <input
            type="button"
            className="stdbtn"
            value={t("common.cancel") ?? undefined}
            onClick={onClose}
          />
        </div>
      </div>
    </div>
  )
}

function PasteConditionCreator({
  fields,
  onApply,
}: {
  fields: string[]
  onApply: (condition: Condition) => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)

  if (!open) {
    return (
      <div className="sub-form-grid">
        <label>{t("subscriptions.pasteCondition")}</label>
        <div>
          <input
            type="button"
            className="stdbtn"
            value={t("subscriptions.pasteConditionButton") ?? undefined}
            onClick={() => setOpen(true)}
          />
        </div>
      </div>
    )
  }

  return <PasteConditionPanel fields={fields} onApply={onApply} onClose={() => setOpen(false)} />
}

function PasteConditionPanel({
  fields,
  onApply,
  onClose,
}: {
  fields: string[]
  onApply: (condition: Condition) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const parse = useParseCondition()
  const [syntax, setSyntax] = useState("")
  const [error, setError] = useState<string | null>(null)

  async function apply() {
    setError(null)
    try {
      const result = await parse.mutateAsync({ dsl: syntax, fields })
      onApply(result.condition)
      setSyntax("")
      onClose()
    } catch (e) {
      setError(e instanceof Error ? e.message : String(e))
    }
  }

  return (
    <div className="sub-form-grid">
      <label style={{ alignSelf: "start", paddingTop: 5 }}>
        {t("subscriptions.pasteCondition")}
      </label>
      <div>
        <textarea
          className="stdinput"
          rows={4}
          style={{ width: "100%", boxSizing: "border-box", resize: "vertical", fontFamily: "monospace" }}
          value={syntax}
          placeholder={t("subscriptions.pasteConditionPrompt") ?? undefined}
          onChange={(e) => setSyntax(e.target.value)}
        />
        <p className="sub-form-hint">{t("subscriptions.pasteConditionHint")}</p>
        {error && <p style={{ color: "red" }}>{error}</p>}
        <div style={{ display: "flex", gap: 6, marginTop: 4 }}>
          <input
            type="button"
            className="stdbtn"
            value={t("common.apply") ?? undefined}
            disabled={parse.isPending || !syntax.trim()}
            onClick={() => void apply()}
          />
          <input
            type="button"
            className="stdbtn"
            value={t("common.cancel") ?? undefined}
            onClick={onClose}
          />
        </div>
      </div>
    </div>
  )
}

/** The compact DSL the AI endpoint consumes, rebuilt from the live condition tree for display. */
function conditionToDsl(condition: Condition): unknown {
  switch (condition.kind) {
    case "all":
      return { all: condition.children.map(conditionToDsl) }
    case "any":
      return { any: condition.children.map(conditionToDsl) }
    case "not":
      return { not: conditionToDsl(condition.child) }
    case "rule":
      return {
        field: condition.field,
        operator: condition.operator,
        ...(condition.operator === "is_empty" || condition.operator === "is_not_empty"
          ? {}
          : { value: condition.value }),
      }
  }
}

function formatRuleValue(operator: FieldOperator, value: RuleValue): string {
  if (operator === "older_than" || operator === "newer_than") {
    const secs = Number(value)
    if (Number.isFinite(secs)) {
      if (secs % 86400 === 0) return `${secs / 86400}d`
      if (secs % 3600 === 0) return `${secs / 3600}h`
      if (secs % 60 === 0) return `${secs / 60}m`
      return `${secs}s`
    }
  }
  if (Array.isArray(value)) return value.map((v) => JSON.stringify(v)).join(", ")
  return JSON.stringify(value)
}

/** Human-readable rendering of the same tree, using the interface language's field/operator labels.
 *
 * One clause per line, indented per nesting level, rather than one long sentence of nested
 * parentheses: a real condition — three exclusions of two clauses each — rendered as a single
 * paragraph is a wall of brackets the reader has to re-parse by eye on every visit. The connective
 * sits on the line of the clause it joins, so "并且" reads as belonging to the clause after it. */
function ConditionDescription({ condition }: { condition: Condition }) {
  const { t } = useTranslation()
  const ruleText = (rule: Extract<Condition, { kind: "rule" }>) =>
    `${t(`subscriptions.field.${rule.field}`, { defaultValue: rule.field })} ${t(
      `subscriptions.operator.${rule.operator}`,
    )} ${formatRuleValue(rule.operator, rule.value)}`

  const lines: string[] = []
  const walk = (node: Condition, depth: number) => {
    const pad = "  ".repeat(depth)
    switch (node.kind) {
      case "rule":
        lines.push(pad + ruleText(node))
        return
      case "not":
        lines.push(pad + t("subscriptions.exceptWhen"))
        walk(node.child, depth + 1)
        return
      case "all":
      case "any": {
        const connective = node.kind === "all" ? t("subscriptions.and") : t("subscriptions.or")
        node.children.forEach((child, i) => {
          if (i === 0) {
            walk(child, depth)
            return
          }
          // A plain rule stays on the connective's own line ("并且 评分 不高于 4"); a nested group
          // starts its own block below it, indented one more level, so the two never run together.
          if (child.kind === "rule") {
            lines.push(`${pad}${connective} ${ruleText(child)}`)
            return
          }
          lines.push(pad + connective)
          walk(child, depth + 1)
        })
      }
    }
  }
  walk(condition, 0)
  return <p style={{ margin: "0 0 12px", whiteSpace: "pre-wrap" }}>{lines.join("\n")}</p>
}

function ConditionViewerButton({ condition }: { condition?: Condition }) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)
  const dslText = condition ? JSON.stringify(conditionToDsl(condition), null, 2) : ""

  function copySyntax() {
    void navigator.clipboard
      .writeText(dslText)
      .then(() =>
        toast({
          text: t("Copied to clipboard!") ?? undefined,
          icon: "info",
          hideAfter: 3000,
        }),
      )
      .catch(() =>
        toast({ text: t("Failed to copy.") ?? undefined, icon: "error" }),
      )
  }

  return (
    <>
      <input
        type="button"
        className="stdbtn"
        style={{ minWidth: 0, width: "auto", padding: "0 10px" }}
        value={t("subscriptions.viewCondition") ?? undefined}
        onClick={() => setOpen(true)}
      />
      {open && (
        <Modal onClose={() => setOpen(false)} width={640} textAlign="left">
          <div
            style={{
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              gap: 8,
              marginBottom: 10,
            }}
          >
            <h3 className="ih" style={{ fontSize: "1.1em", margin: 0 }}>
              {t("subscriptions.filterCondition")}
            </h3>
            {condition && (
              // `zIndex` 9700, like every other tooltip inside a modal: `Modal` sits at 9001, so the
              // bubble's own default (1100) would render behind it — an invisible tooltip.
              <Tooltip label={t("subscriptions.copyCondition") ?? ""} zIndex={9700}>
                <button
                  type="button"
                  className="stdbtn"
                  style={ICON_BUTTON_STYLE}
                  aria-label={t("subscriptions.copyCondition") ?? "Copy"}
                  onClick={copySyntax}
                >
                  <i className="fa fa-copy" aria-hidden="true"></i>
                </button>
              </Tooltip>
            )}
          </div>
          {condition ? (
            <>
              <ConditionDescription condition={condition} />
              <p className="sub-form-section" style={{ marginTop: 0 }}>
                {t("subscriptions.conditionSyntax")}
              </p>
              <CodeBlock code={dslText} language="json" />
            </>
          ) : (
            <p>{t("subscriptions.noConditionsTakesAll")}</p>
          )}
        </Modal>
      )}
    </>
  )
}

/** Builds the condition a subscription filters by, from whatever fields its source provides.
 *
 * Available fields come from the source, so one carrying a page count or a publication date becomes
 * filterable on those with no change here. */
export function FieldRuleEditor({
  fields,
  condition,
  tagSuggestions = [],
  onChange,
}: {
  /** Tags the library already holds. Fetched once by the form and passed in, so the tree does not
   *  subscribe per row. */
  tagSuggestions?: string[]
  fields: string[]
  condition: Condition | undefined
  onChange: (next: Condition | undefined) => void
}) {
  const { t } = useTranslation()

  // Said plainly rather than shown as an empty form: a source reporting no fields cannot be filtered
  // this way at all, and an empty form would look like something failed to load.
  if (fields.length === 0) {
    return <p className="sub-form-hint">{t("subscriptions.noFilterableFields")}</p>
  }

  if (!condition) {
    return (
      <div className="condition-editor-card">
        <div className="condition-editor-header">
          <span>{t("subscriptions.filterCondition")}</span>
          <ConditionViewerButton condition={condition} />
        </div>
        <AiConditionCreator fields={fields} onApply={onChange} />
        <div className="condition-method-separator" role="separator">
          <span>{t("subscriptions.or")}</span>
        </div>
        <PasteConditionCreator fields={fields} onApply={onChange} />
        <div className="condition-method-separator" role="separator">
          <span>{t("subscriptions.or")}</span>
        </div>
        {/* The same two-column row the other two methods use, rather than a bare button under the
            hint: all three start a condition, so all three should look like one of a set. */}
        <div className="sub-form-grid">
          <label>{t("subscriptions.manualCondition")}</label>
          <div>
            <input
              type="button"
              className="stdbtn"
              value={t("subscriptions.addCondition") ?? undefined}
              onClick={() => onChange({ kind: "all", children: [defaultRule(fields)] })}
            />
          </div>
        </div>
        <p className="sub-form-hint">{t("subscriptions.noConditionsTakesAll")}</p>
      </div>
    )
  }

  // The root is always shown as a group, even when it holds one thing. Otherwise a root that became
  // a single rule or an exclusion would have nowhere to change all/any and nowhere to add a sibling —
  // a dead end reachable just by inverting the top-level condition.
  const root: Extract<Condition, { kind: "all" | "any" }> =
    condition.kind === "all" || condition.kind === "any"
      ? condition
      : { kind: "all", children: [condition] }

  return (
    <div className="condition-editor-card">
      <div className="condition-editor-header">
        <span>{t("subscriptions.filterCondition")}</span>
        <ConditionViewerButton condition={condition} />
      </div>
      <AiConditionCreator fields={fields} onApply={onChange} />
      <div className="condition-method-separator" role="separator">
        <span>{t("subscriptions.or")}</span>
      </div>
      <PasteConditionCreator fields={fields} onApply={onChange} />
      <div className="condition-method-separator" role="separator">
        <span>{t("subscriptions.or")}</span>
      </div>
      <ConditionNode
        fields={fields}
        tagSuggestions={tagSuggestions}
        node={root}
        depth={0}
        onChange={(next) => onChange(next)}
        // The root's remove clears the whole condition, which is a different act from removing one
        // child — labelled separately so they are not confused for each other.
        onRemove={() => onChange(undefined)}
        removeLabelKey="subscriptions.clearAllConditions"
      />
    </div>
  )
}

export { OPERATORS }
