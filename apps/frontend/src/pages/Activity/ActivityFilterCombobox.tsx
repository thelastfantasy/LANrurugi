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

/** This combobox's own internal item values are prefixed to tell the two independent facet
 * dimensions (action type vs. actor) apart within a single flat selection array — the two raw
 * value spaces aren't guaranteed disjoint (nothing stops a future `action_types` constant from
 * textually colliding with an `actor_key` shape), and `Combobox.Root`'s own `value`/`items` need
 * one flat array either way once both dimensions share one control. Stripped back off before
 * being handed to `onActionTypesChange`/`onActorsChange` below — callers never see the prefix. */
const ACTION_PREFIX = "action:"
const ACTOR_PREFIX = "actor:"
const OUTCOME_PREFIX = "outcome:"

/** Fixed two-value candidate list — unlike action types/actors, there's no facets endpoint to ask
 * (`Outcome` only ever has these two variants, see that type's own docs), so this combobox's own
 * "操作状态" group is hardcoded rather than derived from `facets`. */
const OUTCOME_VALUES = ["success", "failure"] as const

/** Single merged multi-select filter control for the Activity page — one combobox, exactly two
 * flat groups ("操作"/"用户"), replacing what was originally two separate comboboxes
 * (`ActionTypeCombobox`/`ActorCombobox`) per direct user feedback that they belonged together as
 * one wider control rather than two side-by-side narrow ones — and, per further feedback on an
 * earlier version of this merge, deliberately *not* further sub-grouped by action-type namespace
 * or actor kind either: each real value's own colored chip (see `renderChip` below) already
 * distinguishes it from its neighbors, so a second layer of sub-headers inside each of the two
 * groups added visual noise (`splitActionTypeNamespace`-derived sub-groups, an `actorGroupDefs`
 * list) without actually making anything easier to scan.
 *
 * Each selected value — of either kind — renders as its own removable, colored chip
 * (`activityColors.ts`); which of the two groups a chip's value came from is only visible from its
 * own color, not tracked as a separate visual lane. A token actor's chip/row additionally carries
 * a hover `Tooltip` showing its real id and role — the visible label is just its display name,
 * which alone doesn't disambiguate same-named or revoked tokens. */
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
  /** A single combined callback rather than separate per-dimension callbacks — one Base UI
   * `onValueChange` event (e.g. picking an action-type chip) always changes exactly one dimension,
   * but the *other* dimensions' own callbacks still need to fire in the same tick to keep
   * `combinedValue` in sync. Independent callbacks, each closing over the caller's own `filter`
   * state and calling `onFilterChange({ ...filter, ... })`, read the SAME stale `filter` when
   * called back-to-back in one handler — React doesn't re-run the closure between them — so a
   * later call's spread silently discards an earlier call's change. Confirmed live (before this had
   * a third dimension): selecting an action-type chip never appeared in the input at all, because
   * the immediately-following (no-op, empty-array) actor callback overwrote it a tick later. One
   * callback receiving every dimension's next array at once lets the caller merge them into state
   * in a single update instead. */
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

  function actorLabel(facet: ActivityFacetActor): ReactNode {
    if (facet.kind === "token" && facet.id && !tokensById.has(facet.id)) {
      // A revoked token has no cached name left anywhere (`describe_actor_facet`'s own docs), so
      // `display_name` here is just the bare UUID — its first hyphen-delimited segment (8 hex
      // chars) is plenty to tell two revoked tokens apart without a 36-char id blowing out the
      // chip's width, the same "enough to usually disambiguate" convention `ShortId` uses
      // elsewhere in this app for the same reason. `fa-user-slash` pairs with `fa-user-shield`
      // below (both `fa-user-*`) as the "no longer valid" counterpart to that "full access" glyph.
      const shortId = facet.display_name.split("-")[0]
      const revokedText = t("activity.revokedTokenName", { name: shortId }) ?? `${shortId} (revoked)`
      return (
        <>
          <i className="fas fa-user-slash activity-chip-danger-icon" aria-hidden="true"></i> {revokedText}
        </>
      )
    }
    // `fa-user-shield` matches `ActivityRow.tsx`'s own reasoning — same glyph
    // `Settings/ApiTokensSection.tsx`'s `RoleMarker` already uses for a token's full-access admin
    // role, reused here since Session is exactly that same case.
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
