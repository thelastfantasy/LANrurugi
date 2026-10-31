import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { ValidationError } from "@/api/client"
import {
  useCategories,
  useCreateSubscription,
  useStats,
  useUpdateSubscription,
} from "@/api/hooks"
import type {
  ConflictPolicy,
  SourceChangePolicy,
  SourceRemovalPolicy,
  SubscriptionBody,
  SubscriptionSource,
} from "@/api/types"
import { RadioGroup, RadioItem, Switch } from "@/components/common-ui/Form"
import { TagInput } from "@/components/Form"
import { toast } from "@/toast"

import { FieldRuleEditor, TAG_NAMESPACES } from "./FieldRuleEditor"
import { SourcePlugins } from "./SourcePlugins"

/** Interval choices offered in the form. The server still enforces each source's own floor, so a
 * choice below it is refused with its reason rather than silently raised — a user who believes
 * checks happen hourly while they happen daily would misread every later result. */
const INTERVAL_CHOICES = [
  { secs: 3600, labelKey: "subscriptions.everyHour" },
  { secs: 6 * 3600, labelKey: "subscriptions.everySixHours" },
  { secs: 12 * 3600, labelKey: "subscriptions.everyTwelveHours" },
  { secs: 86400, labelKey: "subscriptions.daily" },
  { secs: 7 * 86400, labelKey: "subscriptions.weekly" },
]

/** Shared by every radio group in the form, so the choices all sit at one indent.
 *
 * A style object rather than a class because `RadioGroup` deliberately omits `className` from its
 * props — passing one through would mean changing a shared component for this page's layout. */
/** The interval choices alone, laid out in two columns — they are short and there are five of them,
 * so a single column wasted the width beside them. Column-first, so reading down the left then the
 * right keeps them in ascending order. */
const INTERVAL_CHOICES_STYLE = {
  display: "grid",
  gridTemplateColumns: "repeat(2, minmax(0, 1fr))",
  gridTemplateRows: "repeat(3, auto)",
  gridAutoFlow: "column",
  gap: "3px 12px",
  marginLeft: 12,
} as const

const CHOICES_STYLE = {
  display: "flex",
  flexDirection: "column",
  gap: 3,
  marginLeft: 12,
} as const

/** A name describing what the subscription actually follows.
 *
 * Suggested rather than imposed: it fills the field so a subscription can be created without stopping
 * to invent a label, and stops updating the moment the user types their own. Returns an empty string
 * when there is nothing to describe yet, so the field simply stays empty rather than showing a name
 * that says nothing. */
function suggestedName(draft: SubscriptionBody): string {
  const listing = draft.criteria.listing_url?.trim()
  if (listing) {
    try {
      const url = new URL(listing.startsWith("http") ? listing : `https://${listing}`)
      // The query is what distinguishes one listing from another on the same site; the host alone
      // would name every such subscription identically.
      const search = url.searchParams.get("f_search")
      return search ? `${url.hostname}: ${search}` : url.hostname + url.pathname
    } catch {
      // Not a parseable URL yet — the user is probably still typing it.
      return listing
    }
  }

  return ""
}

/** Trim and deduplicate a comma-separated tag list, preserving the first occurrence's order. */
function uniqueTags(raw: string): string[] {
  const seen = new Set<string>()
  const out: string[] = []
  for (const part of raw.split(",")) {
    const tag = part.trim()
    if (tag && !seen.has(tag)) {
      seen.add(tag)
      out.push(tag)
    }
  }
  return out
}

/** Shared by create and edit — `subscriptionId` set means edit. */
export function SubscriptionForm({
  sources,
  initial,
  subscriptionId,
  onDraftChange,
  onDone,
}: {
  sources: SubscriptionSource[]
  initial: SubscriptionBody
  subscriptionId?: string
  /** Reports the draft upward so a preview alongside can run against what is currently typed. */
  onDraftChange?: (draft: SubscriptionBody) => void
  onDone: () => void
}) {
  const { t } = useTranslation()
  const create = useCreateSubscription()
  const update = useUpdateSubscription(subscriptionId ?? "")
  const isEdit = subscriptionId != null

  const [draft, setDraft] = useState<SubscriptionBody>(initial)
  const [formError, setFormError] = useState<string | null>(null)
  // The create form and an edit form can be open at once, so ids must not collide — otherwise a
  // label would focus the other form's control.
  const formId = subscriptionId ?? "new"

  // Reported whenever the draft changes, so a preview beside the form tracks what is typed without
  // the form knowing a preview exists.
  //
  // The ref guard matters: the parent stores what it receives, which re-renders this form, which runs
  // the effect again. Comparing by value rather than by identity stops that at one round — the draft
  // object is rebuilt on every keystroke, so its identity always differs even when nothing did.
  const lastReported = useRef<string>("")
  useEffect(() => {
    const encoded = JSON.stringify(draft)
    if (encoded === lastReported.current) return
    lastReported.current = encoded
    onDraftChange?.(draft)
    // `onDraftChange` is a parent setState and stable; including it would loop.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [draft])
  /** Once the user names it themselves, the suggestion stops overwriting what they wrote. An existing
   *  subscription counts as already named — its name is the user's, whatever it says. */
  const [nameEdited, setNameEdited] = useState(subscriptionId != null)
  /** A listing URL replaces the search this extension would otherwise build from the criteria. */
  const suggestion = suggestedName(draft)
  // Suggests only tags the library actually holds: a list taken from the source would offer tags no
  // local archive carries, and a rule built from one would quietly match nothing.
  const stats = useStats()
  const categories = useCategories()
  const tagSuggestions = useMemo(
    () => (stats.data ?? []).map((s) => (s.namespace ? `${s.namespace}:${s.text}` : s.text)),
    [stats.data],
  )

  const selectedSource = sources.find((s) => s.namespace === draft.source)
  // The source's own fields, plus a field per tag namespace where the source reports tags at all —
  // a namespace is only filterable if its tags are there to read.
  const availableFields = useMemo(() => {
    const reported = selectedSource?.candidate_fields ?? []
    if (!reported.includes("tags")) return reported
    return [...reported, ...TAG_NAMESPACES]
  }, [selectedSource])

  const pending = create.isPending || update.isPending

  async function submit() {
    setFormError(null)
    const body: SubscriptionBody = {
      ...draft,
      // The suggestion is what the field shows, so it is what gets saved unless the user replaced it.
      name: nameEdited ? draft.name : suggestion,
    }
    try {
      if (isEdit) {
        await update.mutateAsync(body)
        toast({ text: t("subscriptions.saved") ?? undefined, icon: "success" })
      } else {
        await create.mutateAsync(body)
        toast({ text: t("subscriptions.created") ?? undefined, icon: "success" })
      }
      onDone()
    } catch (e) {
      // The server refuses a too-frequent interval or a source without discovery, and says why.
      // Surfacing that message verbatim is the point — a generic failure would hide the one piece
      // of information that lets the user fix the input.
      setFormError(e instanceof ValidationError ? e.message : String(e))
    }
  }

  return (
    // The panel centres its own contents; the form overrides that for everything inside it, so the
    // fields, the section headings and the buttons all share one left edge instead of three.
    <div className="sub-form">
      {/* Scrolls on its own, with the buttons left outside it: a few conditions are enough to push
          the form past the viewport, and "create" scrolling out of reach is the one control that must
          not. */}
      <div className="sub-form-fields">
        {/* First, because it governs everything below it: a subscription can be written now and
            switched on once its preview looks right. Only offered while creating — an existing one is
            enabled and disabled from its row in the list, where its current state is visible. */}
        {!isEdit && (
          <label className="sub-form-enable">
            {/* Before the switch, which carries its own ON/OFF: after it the two ran together as
                "ON enabled". */}
            <span>{t("subscriptions.enableLabel")}</span>
            <Switch
              checked={draft.enabled ?? true}
              onCheckedChange={(enabled) => setDraft({ ...draft, enabled })}
            />
          </label>
        )}

      {/* Two columns so every control starts at the same x: with one label-then-input per line, each
          input began wherever its label happened to end, and nothing lined up. */}
      <div className="sub-form-grid">
        <label htmlFor={`sub-name-${formId}`}>{t("subscriptions.name")}</label>
        <input
          id={`sub-name-${formId}`}
          className="stdinput"
          value={nameEdited ? draft.name : suggestion}
          onChange={(e) => {
            setNameEdited(true)
            setDraft({ ...draft, name: e.target.value })
          }}
        />

        <label htmlFor={`sub-source-${formId}`}>{t("subscriptions.source")}</label>
        <div>
          <select
            id={`sub-source-${formId}`}
            className="stdinput"
            value={draft.source}
            onChange={(e) => setDraft({ ...draft, source: e.target.value })}
          >
            {sources.map((s) => (
              <option key={s.namespace} value={s.namespace}>
                {s.namespace}
              </option>
            ))}
          </select>
          {selectedSource?.description && (
            // Below the control rather than beside it: this text is a sentence or two, and trailing
            // it on the same line pushed the layout wider than the panel.
            <p className="sub-form-hint">{selectedSource.description}</p>
          )}
          {selectedSource && (
            <SourcePlugins
              source={selectedSource}
              credentials={draft.credentials}
              onCredentialsChange={(credentials) => setDraft({ ...draft, credentials })}
            />
          )}
        </div>

        <label htmlFor={`sub-listing-${formId}`}>{t("subscriptions.listingUrl")}</label>
        <div>
          <input
            id={`sub-listing-${formId}`}
            className="stdinput"
            value={draft.criteria.listing_url ?? ""}
            placeholder={t("subscriptions.listingUrlPlaceholder") ?? undefined}
            onChange={(e) =>
              setDraft({
                ...draft,
                criteria: {
                  ...draft.criteria,
                  // Empty means "not set" rather than an empty URL, which the source would try to fetch.
                  listing_url: e.target.value.trim() || undefined,
                },
              })
            }
          />
          <p className="sub-form-hint">{t("subscriptions.listingUrlHint")}</p>
        </div>

      </div>

      <p className="sub-form-section">{t("subscriptions.filterRules")}</p>
      <FieldRuleEditor
        fields={availableFields}
        tagSuggestions={tagSuggestions}
        condition={draft.filters.condition}
        onChange={(condition) =>
          setDraft({ ...draft, filters: { ...draft.filters, condition } })
        }
      />

      <p className="sub-form-section">{t("subscriptions.checkEvery")}</p>
      <RadioGroup
        value={String(draft.interval_secs)}
        onValueChange={(v) => setDraft({ ...draft, interval_secs: Number(v) })}
        name={`subscription-interval-${subscriptionId ?? "new"}`}
        // Two columns rather than one: five short labels in a single column left a tall, mostly empty
        // strip beside the form's other controls.
        style={INTERVAL_CHOICES_STYLE}
      >
        {INTERVAL_CHOICES.map((choice) => (
          <RadioItem key={choice.secs} value={String(choice.secs)}>
            {t(choice.labelKey)}
            {selectedSource?.minimum_secs != null &&
              choice.secs < selectedSource.minimum_secs && (
                <span style={{ color: "#c79121" }}>
                  {" "}
                  {t("subscriptions.belowSourceMinimum")}
                </span>
              )}
          </RadioItem>
        ))}
      </RadioGroup>

      <p className="sub-form-section">{t("subscriptions.whenMatchesAreFound")}</p>
      <RadioGroup
        value={draft.auto_download ? "auto" : "confirm"}
        onValueChange={(v) => setDraft({ ...draft, auto_download: v === "auto" })}
        name={`subscription-auto-download-${subscriptionId ?? "new"}`}
        style={CHOICES_STYLE}
      >
        <RadioItem value="confirm">{t("subscriptions.waitForConfirmation")}</RadioItem>
        <RadioItem value="auto">{t("subscriptions.downloadAutomatically")}</RadioItem>
      </RadioGroup>

      <p className="sub-form-section">{t("subscriptions.whenFilenamesCollide")}</p>
      <RadioGroup
        value={draft.conflict_policy}
        onValueChange={(v) => setDraft({ ...draft, conflict_policy: v as ConflictPolicy })}
        name={`subscription-conflict-policy-${subscriptionId ?? "new"}`}
        style={CHOICES_STYLE}
      >
        <RadioItem value="ask">{t("subscriptions.conflictAsk")}</RadioItem>
        <RadioItem value="auto_rename">{t("subscriptions.conflictAutoRename")}</RadioItem>
        <RadioItem value="overwrite">{t("subscriptions.conflictOverwrite")}</RadioItem>
        <RadioItem value="discard">{t("subscriptions.conflictDiscard")}</RadioItem>
      </RadioGroup>
      <p className="sub-form-hint" style={{ marginLeft: 12 }}>
        {t("subscriptions.conflictHint")}
      </p>

      <p className="sub-form-section">{t("subscriptions.whenSourceChanges")}</p>
      <RadioGroup
        value={draft.on_source_changed}
        onValueChange={(v) => setDraft({ ...draft, on_source_changed: v as SourceChangePolicy })}
        name={`subscription-on-source-changed-${subscriptionId ?? "new"}`}
        style={CHOICES_STYLE}
      >
        <RadioItem value="notify_only">{t("subscriptions.changeNotifyOnly")}</RadioItem>
        <RadioItem value="refetch_metadata">{t("subscriptions.changeRefetchMetadata")}</RadioItem>
        <RadioItem value="ignore">{t("subscriptions.changeIgnore")}</RadioItem>
      </RadioGroup>

      <p className="sub-form-section">{t("subscriptions.whenSourceRemoves")}</p>
      <RadioGroup
        value={draft.on_source_removed}
        onValueChange={(v) => setDraft({ ...draft, on_source_removed: v as SourceRemovalPolicy })}
        name={`subscription-on-source-removed-${subscriptionId ?? "new"}`}
        style={CHOICES_STYLE}
      >
        <RadioItem value="mark_only">{t("subscriptions.removalMarkOnly")}</RadioItem>
        <RadioItem value="notify">{t("subscriptions.removalNotify")}</RadioItem>
        <RadioItem value="ignore">{t("subscriptions.removalIgnore")}</RadioItem>
      </RadioGroup>
      <p className="sub-form-hint" style={{ marginLeft: 12 }}>
        {t("subscriptions.removalNeverDeletes")}
      </p>

      <p className="sub-form-section">{t("subscriptions.metadataHandling")}</p>
      <div className="sub-form-grid">
        <label htmlFor={`sub-category-${formId}`}>{t("subscriptions.targetCategory")}</label>
        <div>
          <select
            id={`sub-category-${formId}`}
            className="stdinput"
            value={draft.target_category ?? ""}
            onChange={(e) =>
              setDraft({ ...draft, target_category: e.target.value || undefined })
            }
          >
            <option value="">{t("subscriptions.categoryNone")}</option>
            {(categories.data ?? []).map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
          <p className="sub-form-hint">{t("subscriptions.categoryHint")}</p>
        </div>

        {/* `alignSelf` because the tag editor grows to two/three lines once a tag is added;
            baseline alignment would drag this label down with its first line. */}
        <label style={{ alignSelf: "start", paddingTop: 5 }}>{t("subscriptions.addTags")}</label>
        <div>
          <TagInput
            value={(draft.metadata_tags ?? []).join(", ")}
            suggestions={tagSuggestions}
            onChange={(next) => setDraft({ ...draft, metadata_tags: uniqueTags(next) })}
          />
          <p className="sub-form-hint">{t("subscriptions.addTagsHint")}</p>
        </div>
      </div>

      {formError && (
        <p className="error" style={{ color: "red" }}>
          {formError}
        </p>
      )}


      </div>

      <div className="sub-form-actions">
        <input
          type="button"
          className="stdbtn"
          disabled={pending || !(nameEdited ? draft.name : suggestion).trim()}
          value={(isEdit ? t("subscriptions.save") : t("subscriptions.create")) ?? undefined}
          onClick={() => void submit()}
        />
        <input
          type="button"
          className="stdbtn"
          value={t("common.cancel") ?? undefined}
          onClick={onDone}
        />
      </div>
    </div>
  )
}
