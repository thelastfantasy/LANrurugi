import { useState } from "react"

import { CUSTOM_COLUMN_PREFIX, DEFAULT_CUSTOM_COLUMNS } from "@/lib/storageKeys"

export function useCustomColumnNamespace(index: number): [string, (v: string) => void] {
  const key = `${CUSTOM_COLUMN_PREFIX}${index}`
  const [namespace, setNamespaceState] = useState(
    () => localStorage.getItem(key) ?? DEFAULT_CUSTOM_COLUMNS[index - 1] ?? `Header ${index}`,
  )
  const setNamespace = (v: string) => {
    setNamespaceState(v)
    localStorage.setItem(key, v)
  }
  return [namespace, setNamespace]
}
