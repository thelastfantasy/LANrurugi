/** TanStack Query hooks for share management (`/api/shares*`). */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"

import { fetchJson, sendJson } from "@/api/client"

import type {
  HelpingImport,
  NewShare,
  ShareCodeInfo,
  ShareDetail,
  ShareFields,
  ShareItemRef,
  ShareSettings,
  ShareSummary,
} from "./types"

const SHARES_KEY = ["shares"] as const

export function useShares(status: "active" | "all" = "all") {
  return useQuery({
    queryKey: [...SHARES_KEY, "list", status],
    queryFn: () => fetchJson<ShareSummary[]>(`/shares?status=${status}`),
  })
}

export function useShare(id: string | null) {
  return useQuery({
    queryKey: [...SHARES_KEY, "detail", id],
    queryFn: () => fetchJson<ShareDetail>(`/shares/${encodeURIComponent(id ?? "")}`),
    enabled: id !== null,
  })
}

export function useShareCode(id: string | null) {
  return useQuery({
    queryKey: [...SHARES_KEY, "code", id],
    queryFn: () => fetchJson<ShareCodeInfo>(`/shares/${encodeURIComponent(id ?? "")}/code`),
    enabled: id !== null,
  })
}

export function useSharesByItem(itemId: string | null) {
  return useQuery({
    queryKey: [...SHARES_KEY, "by-item", itemId],
    queryFn: () => fetchJson<ShareSummary[]>(`/shares/by-item/${encodeURIComponent(itemId ?? "")}`),
    enabled: itemId !== null,
  })
}

function useShareMutation<V, R>(fn: (vars: V) => Promise<R>) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: fn,
    onSuccess: () => queryClient.invalidateQueries({ queryKey: SHARES_KEY }),
  })
}

export function useCreateShare() {
  return useShareMutation((body: NewShare) => sendJson<ShareDetail>("POST", "/shares", body))
}

export function useUpdateShare() {
  return useShareMutation(({ id, fields }: { id: string; fields: ShareFields }) =>
    sendJson<ShareDetail>("PATCH", `/shares/${encodeURIComponent(id)}`, fields),
  )
}

export function useAddShareItems() {
  return useShareMutation(({ id, items, update }: { id: string; items: ShareItemRef[]; update?: ShareFields }) =>
    sendJson<ShareDetail>("POST", `/shares/${encodeURIComponent(id)}/items`, { items, update }),
  )
}

export function useRemoveShareItems() {
  return useShareMutation(({ id, items }: { id: string; items: ShareItemRef[] }) =>
    sendJson<ShareDetail>("DELETE", `/shares/${encodeURIComponent(id)}/items`, { items }),
  )
}

/** Replaces the item order. The share's code, claims and limits are untouched. */
export function useReorderShareItems() {
  return useShareMutation(({ id, items }: { id: string; items: ShareItemRef[] }) =>
    sendJson<ShareDetail>("PUT", `/shares/${encodeURIComponent(id)}/items`, { items }),
  )
}

export function useRevokeShare() {
  return useShareMutation((id: string) => sendJson<ShareDetail>("POST", `/shares/${encodeURIComponent(id)}/revoke`))
}

export function useShareSettings() {
  return useQuery({
    queryKey: [...SHARES_KEY, "settings"],
    queryFn: () => fetchJson<ShareSettings>("/shares/settings"),
  })
}

export function useSaveShareSettings() {
  return useShareMutation((fields: ShareSettings) => sendJson<ShareSettings>("PUT", "/shares/settings", fields))
}

const HELPING_KEY = ["shares", "imports", "helping"] as const

/** Imports through which this instance helps deliver other instances' shares (T097). */
export function useHelpingImports() {
  return useQuery({
    queryKey: HELPING_KEY,
    queryFn: () => fetchJson<HelpingImport[]>("/shares/imports?helping=1"),
    refetchInterval: 30_000,
  })
}

export function useUpdateHelping() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, ...fields }: { id: string; help_deliver?: boolean; help_hidden?: boolean }) =>
      sendJson<HelpingImport>("PATCH", `/shares/imports/${encodeURIComponent(id)}`, fields),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: HELPING_KEY }),
  })
}
