type CarouselMode = "ondeck" | "random" | "inbox" | "untagged"

export const PAGE_SIZE = 100

export const NEW_ONLY = "NEW_ONLY"

export const UNTAGGED_ONLY = "UNTAGGED_ONLY"

export const CATEGORY_BUTTON_CAP = 10

export const CAROUSEL_ICON: Record<CarouselMode, string> = {
  ondeck: "📚",
  random: "🎲",
  inbox: "📥",
  untagged: "🏷️",
}
