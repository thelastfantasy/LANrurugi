import { useState } from "react"
import { useTranslation } from "react-i18next"

import { ValidationError } from "@/api/client"
import {
  useCreateSubscription,
  useDeleteSubscription,
  useSetSubscriptionState,
  useSubscriptions,
  useSubscriptionSources,
} from "@/api/hooks"
import type { CreditPolicy, Subscription, SubscriptionBody, SubscriptionSource } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { RadioGroup, RadioItem } from "@/components/common-ui/Form"
import { CollapsibleSection } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { toast } from "@/toast"

import { ICON_BUTTON_STYLE } from "../Upload/shared"

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

function emptyBody(source: string, intervalSecs: number): SubscriptionBody {
  return {
    name: "",
    source,
    criteria: {},
    filters: {},
    interval_secs: intervalSecs,
    enrich_metadata: true,
    // Off by default (FR-007a): a brand-new subscription's rules are usually still being tuned.
    auto_download: false,
    credit_policy: "pause",
  }
}

function splitList(raw: string): string[] {
  return raw
    .split(",")
    .map((s) => s.trim())
    .filter(Boolean)
}

export function SubscriptionsSection() {
  const { t } = useTranslation()
  const subscriptions = useSubscriptions()
  const sources = useSubscriptionSources()

  return (
    <CollapsibleSection
      id="subscriptions"
      icon="fa-rss"
      title={t("subscriptions.title") ?? "Subscriptions"}
    >
      {sources.isLoading || subscriptions.isLoading ? (
        <p>{t("common.loading")}</p>
      ) : (sources.data?.length ?? 0) === 0 ? (
        // Stated rather than left as an empty form: a source only qualifies if its extension can
        // discover, and silently showing nothing would look like a bug.
        <p>{t("subscriptions.noSourcesAvailable")}</p>
      ) : (
        <SubscriptionsBody
          subscriptions={subscriptions.data ?? []}
          sources={sources.data ?? []}
        />
      )}
    </CollapsibleSection>
  )
}

function SubscriptionsBody({
  subscriptions,
  sources,
}: {
  subscriptions: Subscription[]
  sources: SubscriptionSource[]
}) {
  const { t } = useTranslation()
  const create = useCreateSubscription()
  const remove = useDeleteSubscription()
  const setState = useSetSubscriptionState()

  const firstSource = sources[0]
  const [draft, setDraft] = useState<SubscriptionBody>(() =>
    emptyBody(firstSource?.namespace ?? "", firstSource?.suggested_secs ?? 6 * 3600),
  )
  const [requiredRaw, setRequiredRaw] = useState("")
  const [excludedRaw, setExcludedRaw] = useState("")
  const [formError, setFormError] = useState<string | null>(null)

  const selectedSource = sources.find((s) => s.namespace === draft.source)

  async function submit() {
    setFormError(null)
    const body: SubscriptionBody = {
      ...draft,
      filters: {
        ...draft.filters,
        required_tags: splitList(requiredRaw),
        excluded_tags: splitList(excludedRaw),
      },
    }
    try {
      await create.mutateAsync(body)
      toast({ text: t("subscriptions.created") ?? undefined, icon: "success" })
      setDraft(emptyBody(draft.source, draft.interval_secs))
      setRequiredRaw("")
      setExcludedRaw("")
    } catch (e) {
      // The server refuses a too-frequent interval or a source without discovery, and says why.
      // Surfacing that message verbatim is the point — a generic failure would hide the one piece
      // of information that lets the user fix the input.
      setFormError(e instanceof ValidationError ? e.message : String(e))
    }
  }

  return (
    <div>
      <table className="itg" style={{ width: "100%" }}>
        <thead>
          <tr className="jtr0">
            <th>{t("subscriptions.name")}</th>
            <th>{t("subscriptions.source")}</th>
            <th>{t("subscriptions.interval")}</th>
            <th>{t("subscriptions.state")}</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {subscriptions.length === 0 && (
            <tr className="gtr1">
              <td colSpan={5}>{t("subscriptions.none")}</td>
            </tr>
          )}
          {subscriptions.map((s) => (
            <tr key={s.id} className="gtr1">
              <td>{s.name}</td>
              <td>{s.source}</td>
              <td>{Math.round(s.interval_secs / 3600)}h</td>
              <td>
                {s.state.state === "paused" ? (
                  // A pause is the system's doing, so it owes the user both the reason and a way
                  // back — unlike "disabled", which the user chose.
                  <span style={{ color: "#c79121" }}>
                    {t("subscriptions.pausedInsufficientCredit")}{" "}
                    <input
                      type="button"
                      className="stdbtn"
                      value={t("subscriptions.resume") ?? undefined}
                      onClick={() =>
                        void setState.mutateAsync({ id: s.id, action: "resume" })
                      }
                    />
                  </span>
                ) : (
                  <label>
                    <input
                      type="checkbox"
                      className="fa"
                      checked={s.state.state === "enabled"}
                      onChange={(e) =>
                        void setState.mutateAsync({
                          id: s.id,
                          action: e.target.checked ? "enable" : "disable",
                        })
                      }
                    />{" "}
                    {s.state.state === "enabled"
                      ? t("subscriptions.enabled")
                      : t("subscriptions.disabled")}
                  </label>
                )}
              </td>
              <td>
                <Tooltip label={t("subscriptions.delete") ?? ""}>
                  <button
                    type="button"
                    className="stdbtn"
                    style={ICON_BUTTON_STYLE}
                    onClick={() => {
                      void confirmDialog(
                        t("subscriptions.confirmDelete", { name: s.name }) ?? "",
                      ).then((ok) => {
                        if (ok) void remove.mutateAsync(s.id)
                      })
                    }}
                  >
                    <i className="fa fa-times" aria-hidden="true"></i>
                  </button>
                </Tooltip>
              </td>
            </tr>
          ))}
        </tbody>
      </table>

      <h3 className="ih" style={{ fontSize: "1.0em", margin: "10px 0 6px" }}>
        {t("subscriptions.addNew")}
      </h3>

      <p>
        <label>
          {t("subscriptions.name")}:{" "}
          <input
            className="stdinput"
            value={draft.name}
            onChange={(e) => setDraft({ ...draft, name: e.target.value })}
          />
        </label>
      </p>

      <p>
        <label>
          {t("subscriptions.source")}:{" "}
          <select
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
        </label>
        {selectedSource?.description && (
          <span style={{ marginLeft: 8, opacity: 0.8 }}>{selectedSource.description}</span>
        )}
      </p>

      <p>
        <label>
          {t("subscriptions.creator")}:{" "}
          <input
            className="stdinput"
            value={draft.criteria.creator ?? ""}
            onChange={(e) =>
              setDraft({ ...draft, criteria: { ...draft.criteria, creator: e.target.value } })
            }
          />
        </label>
      </p>

      <p>
        <label>
          {t("subscriptions.requiredTags")}:{" "}
          <input
            className="stdinput"
            value={requiredRaw}
            placeholder={t("subscriptions.commaSeparated") ?? undefined}
            onChange={(e) => setRequiredRaw(e.target.value)}
          />
        </label>
      </p>

      <p>
        <label>
          {t("subscriptions.excludedTags")}:{" "}
          <input
            className="stdinput"
            value={excludedRaw}
            placeholder={t("subscriptions.commaSeparated") ?? undefined}
            onChange={(e) => setExcludedRaw(e.target.value)}
          />
        </label>
      </p>

      <p style={{ margin: "10px 0 6px" }}>{t("subscriptions.checkEvery")}</p>
      <RadioGroup
        value={String(draft.interval_secs)}
        onValueChange={(v) => setDraft({ ...draft, interval_secs: Number(v) })}
        name="subscription-interval"
        style={{ display: "flex", flexDirection: "column", gap: 3 }}
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

      <p style={{ margin: "10px 0 6px" }}>{t("subscriptions.whenMatchesAreFound")}</p>
      <RadioGroup
        value={draft.auto_download ? "auto" : "confirm"}
        onValueChange={(v) => setDraft({ ...draft, auto_download: v === "auto" })}
        name="subscription-auto-download"
        style={{ display: "flex", flexDirection: "column", gap: 3 }}
      >
        <RadioItem value="confirm">{t("subscriptions.waitForConfirmation")}</RadioItem>
        <RadioItem value="auto">{t("subscriptions.downloadAutomatically")}</RadioItem>
      </RadioGroup>

      <p style={{ margin: "10px 0 6px" }}>{t("subscriptions.whenCreditRunsOut")}</p>
      <RadioGroup
        value={draft.credit_policy}
        onValueChange={(v) => setDraft({ ...draft, credit_policy: v as CreditPolicy })}
        name="subscription-credit-policy"
        style={{ display: "flex", flexDirection: "column", gap: 3 }}
      >
        <RadioItem value="pause">{t("subscriptions.pauseSubscription")}</RadioItem>
        <RadioItem value="continue_and_reserve">
          {t("subscriptions.continueAndReserve")}
        </RadioItem>
      </RadioGroup>

      {formError && (
        <p className="error" style={{ color: "red" }}>
          {formError}
        </p>
      )}

      <div style={{ display: "flex", gap: 8, marginTop: 8 }}>
        <input
          type="button"
          className="stdbtn"
          disabled={create.isPending || !draft.name.trim()}
          value={t("subscriptions.create") ?? undefined}
          onClick={() => void submit()}
        />
      </div>
    </div>
  )
}
