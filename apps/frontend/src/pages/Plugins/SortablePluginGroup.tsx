import { useState } from "react"

import { useReorderPlugins } from "@/api/hooks"
import type { PluginInfo } from "@/api/types"
import { SortableList } from "@/components/common-ui/Display"

import { PluginCard } from "./PluginCard"

/** One `type` group's drag-to-reorder plugin list. Local `order` state is seeded from the
 * server's list so a drag reorders instantly; order matters for `findMatchingPlugin`. */
export function SortablePluginGroup({ type, plugins }: { type: PluginInfo["type"]; plugins: PluginInfo[] }) {
  const reorder = useReorderPlugins()
  const serverOrder = plugins.map((p) => p.namespace)
  const serverOrderKey = serverOrder.join(",")

  // Reset during render (not useEffect) when the server list changes for a reason other than our
  // own drag; skipped while a drag-triggered mutation is in flight to avoid a visible snap-back.
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
