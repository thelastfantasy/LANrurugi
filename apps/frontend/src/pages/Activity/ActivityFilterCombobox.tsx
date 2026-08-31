import { Combobox } from "@base-ui/react/combobox"
import { type ReactNode,useMemo } from "react"
import { useTranslation } from "react-i18next"

import { useApiTokens } from "@/api/hooks"
import type { ActivityFacetActionType, ActivityFacetActor, ActivityFacets } from "@/api/types"

import { actionTypeColor, actorKindColor, outcomeColor } from "./activityColors"
import { ActivityComboboxGroup, ActivityComboboxItem, ActivityComboboxShell, type ChipRenderResult } from "./ActivityCombobox"
import { actionTypeLabel, compareActionTypesForDisplay, outcomeLabel } from "./activityTarget"

/** `actor` query values match `AuthContext::trace_label()`/the backend's own `actor_key` shape:
 * `"session"`, `"token:<id>"`, `"system:<subsystem>"`, `"anonymous"`. */
function actorKey(facet: ActivityFacetActor): string {
  if (facet.kind === "token" && facet.id) return `token:${facet.id}`
  if (facet.kind === "system" && facet.id) return `system:${facet.id}`
  return facet.kind
}

/** Prefixes tell the independent facet dimensions apart in one flat selection array (raw value
 * spaces aren't guaranteed disjoint); stripped back off before being handed to callers below. */
const ACTION_PREFIX = "action:"
const ACTOR_PREFIX = "actor:"
const OUTCOME_PREFIX = "outcome:"

/** Fixed two-value candidate list — no facets endpoint to ask, since `Outcome` only ever has
 * these two variants. */
const OUTCOME_VALUES = ["success", "failure"] as const

/** Single merged multi-select filter control for the Activity page — one combobox, two flat
 * groups, distinguished only by each selected value's colored chip. */
export function ActivityFilterCombobox({
  actionTypes,
  actors,
  outcomes,
  onFilterDimensionsChange,
  facets,
}: {
  actionTypes: string[]
  actors: string[]
  outcomes: string[]
  /** One combined callback, not per-dimension ones — separate callbacks closing over stale
   * `filter` state would discard each other's changes when called back-to-back. */
  onFilterDimensionsChange: (actionTypes: string[], actors: string[], outcomes: string[]) => void
  facets: ActivityFacets | undefined
}) {
  const { t } = useTranslation()
  const apiTokens = useApiTokens()
  const tokensById = useMemo(() => new Map((apiTokens.data ?? []).map((token) => [token.id, token])), [apiTokens.data])

  const actionTypeFacets: ActivityFacetActionType[] = useMemo(() => facets?.action_types ?? [], [facets])
  const actorFacets: ActivityFacetActor[] = facets?.actors ?? []

  const sortedActionTypeFacets = useMemo(
    () => [...actionTypeFacets].sort((a, b) => compareActionTypesForDisplay(t, a.value, b.value)),
    [actionTypeFacets, t],
  )

  // A revoked token has no cached name; display_name is the bare UUID, so just its first
  // segment (8 hex chars) disambiguates without blowing out the chip's width.
  function actorLabel(facet: ActivityFacetActor): ReactNode {
    if (facet.kind === "token" && facet.id && !tokensById.has(facet.id)) {
      const shortId = facet.display_name.split("-")[0]
      const revokedText = t("activity.revokedTokenName", { name: shortId }) ?? `${shortId} (revoked)`
      return (
        <>
          <i className="fas fa-user-slash activity-chip-danger-icon" aria-hidden="true"></i> {revokedText}
        </>
      )
    }
    if (facet.kind === "session") {
      return (
        <>
          <i className="fas fa-user-shield" aria-hidden="true"></i> {t("activity.sessionUser") ?? facet.display_name}
        </>
      )
    }
    if (facet.kind === "anonymous") return t("activity.anonymousUser") ?? facet.display_name
    return facet.display_name
  }

  function actorTooltip(facet: ActivityFacetActor) {
    if (facet.kind !== "token" || !facet.id) return undefined
    const token = tokensById.get(facet.id)
    const roleLabel = token ? t(token.role === "guest" ? "Guest" : "Admin") : null
    return (
      <>
        <div style={{ fontFamily: "monospace" }}>{facet.id}</div>
        {roleLabel && <div>{roleLabel}</div>}
        {!token && <div>{t("activity.revoked")}</div>}
      </>
    )
  }

  function actorColorFor(facet: ActivityFacetActor) {
    return actorKindColor(facet.kind, facet.id ?? undefined)
  }

  const allItems = [
    ...actionTypeFacets.map((f) => ACTION_PREFIX + f.value),
    ...actorFacets.map((f) => ACTOR_PREFIX + actorKey(f)),
    ...OUTCOME_VALUES.map((v) => OUTCOME_PREFIX + v),
  ]
  const combinedValue = [
    ...actionTypes.map((v) => ACTION_PREFIX + v),
    ...actors.map((v) => ACTOR_PREFIX + v),
    ...outcomes.map((v) => OUTCOME_PREFIX + v),
  ]

  function handleValueChange(next: string[]) {
    const nextActionTypes = next.filter((v) => v.startsWith(ACTION_PREFIX)).map((v) => v.slice(ACTION_PREFIX.length))
    const nextActors = next.filter((v) => v.startsWith(ACTOR_PREFIX)).map((v) => v.slice(ACTOR_PREFIX.length))
    const nextOutcomes = next.filter((v) => v.startsWith(OUTCOME_PREFIX)).map((v) => v.slice(OUTCOME_PREFIX.length))
    onFilterDimensionsChange(nextActionTypes, nextActors, nextOutcomes)
  }

  function renderChip(prefixedValue: string): ChipRenderResult {
    if (prefixedValue.startsWith(ACTION_PREFIX)) {
      const actionType = prefixedValue.slice(ACTION_PREFIX.length)
      return { content: actionTypeLabel(t, actionType), color: actionTypeColor(actionType) }
    }
    if (prefixedValue.startsWith(OUTCOME_PREFIX)) {
      const status = prefixedValue.slice(OUTCOME_PREFIX.length)
      return { content: outcomeLabel(t, status), color: outcomeColor(status) }
    }
    const key = prefixedValue.slice(ACTOR_PREFIX.length)
    const facet = actorFacets.find((f) => actorKey(f) === key)
    if (!facet) return { content: key, color: actorKindColor("anonymous") }
    return { content: actorLabel(facet), color: actorColorFor(facet), tooltip: actorTooltip(facet) }
  }

  return (
    <Combobox.Root multiple items={allItems} value={combinedValue} onValueChange={handleValueChange}>
      <ActivityComboboxShell
        placeholder={t("activity.searchActionTypesOrActors") ?? ""}
        emptyLabel={t("activity.noMatches") ?? ""}
        renderChip={renderChip}
        wide
      >
        {sortedActionTypeFacets.length > 0 && (
          <ActivityComboboxGroup label={t("activity.filterGroupActionTypes") ?? ""}>
            {sortedActionTypeFacets.map((facet) => (
              <ActivityComboboxItem
                key={ACTION_PREFIX + facet.value}
                value={ACTION_PREFIX + facet.value}
                color={actionTypeColor(facet.value)}
                label={actionTypeLabel(t, facet.value)}
                count={facet.count}
              />
            ))}
          </ActivityComboboxGroup>
        )}

        {actorFacets.length > 0 && (
          <ActivityComboboxGroup label={t("activity.filterGroupActors") ?? ""}>
            {actorFacets.map((facet) => (
              <ActivityComboboxItem
                key={ACTOR_PREFIX + actorKey(facet)}
                value={ACTOR_PREFIX + actorKey(facet)}
                color={actorColorFor(facet)}
                label={actorLabel(facet)}
                count={facet.count}
                tooltip={facet.kind === "token" ? actorTooltip(facet) : undefined}
              />
            ))}
          </ActivityComboboxGroup>
        )}

        <ActivityComboboxGroup label={t("activity.filterGroupOutcomes") ?? ""}>
          {OUTCOME_VALUES.map((status) => (
            <ActivityComboboxItem
              key={OUTCOME_PREFIX + status}
              value={OUTCOME_PREFIX + status}
              color={outcomeColor(status)}
              label={outcomeLabel(t, status)}
            />
          ))}
        </ActivityComboboxGroup>
      </ActivityComboboxShell>
    </Combobox.Root>
  )
}
