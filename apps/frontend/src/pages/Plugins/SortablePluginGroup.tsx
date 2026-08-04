import { useState } from "react"

import { useReorderPlugins } from "@/api/hooks"
import type { PluginInfo } from "@/api/types"
import { SortableList } from "@/components/Display"

import { PluginCard } from "./PluginCard"

/** One `type` group's drag-to-reorder plugin list (additive — legacy has no concept of plugin
 * priority at all). Local `order` state is seeded from (and re-synced with) the server's already
 * priority-sorted list, so a drag reorders instantly without waiting on the round trip, and a
 * background refetch doesn't fight the in-progress drag. On drop, persists the complete new order
 * via `useReorderPlugins` — this matters because `findMatchingPlugin` (Upload.tsx) picks the first
 * URL-pattern match in this exact order, so dragging one plugin above another is what makes it the
 * one actually used for a URL both could handle. */
export function SortablePluginGroup({ type, plugins }: { type: PluginInfo["type"]; plugins: PluginInfo[] }) {
  const reorder = useReorderPlugins()
  const serverOrder = plugins.map((p) => p.namespace)
  const serverOrderKey = serverOrder.join(",")

  // Local `order` state only reflects an in-progress/just-finished drag ahead of the server round
  // trip — reset during render (React's pattern for "adjust state when a prop changes", not a
  // `useEffect`) whenever the server's list changes for a reason other than this component's own
  // drag. Skipped while a drag-triggered mutation is still in flight, so the just-dropped order
  // doesn't visibly snap back to the pre-drag server value.
  const [order, setOrder] = useState(serverOrder)
  const [syncedKey, setSyncedKey] = useState(serverOrderKey)
  if (serverOrderKey !== syncedKey && !reorder.isPending) {
    setOrder(serverOrder)
    setSyncedKey(serverOrderKey)
  }

  const byNamespace = new Map(plugins.map((p) => [p.namespace, p]))
  const orderedPlugins = order.map((namespace) => byNamespace.get(namespace)).filter((p): p is PluginInfo => !!p)

  return (
    <SortableList
      items={orderedPlugins}
      getId={(p) => p.namespace}
      onReorder={(next) => {
        setOrder(next)
        reorder.mutate({ type, order: next })
      }}
      renderItem={(plugin, dragHandleProps) => <PluginCard plugin={plugin} dragHandleProps={dragHandleProps} />}
    />
  )
}
