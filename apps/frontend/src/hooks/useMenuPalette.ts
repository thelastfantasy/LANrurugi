import { useSettings } from "@/api/hooks"
import { DEFAULT_THEME_ID, MENU_PALETTE } from "@/theme"

export function useMenuPalette() {
  const settings = useSettings()
  const themeId = settings.data?.theme ?? DEFAULT_THEME_ID
  return MENU_PALETTE[themeId as keyof typeof MENU_PALETTE] ?? MENU_PALETTE[DEFAULT_THEME_ID]
}
