import type { CSSProperties } from "react"
import type { IconType } from "react-icons"
import { FaBandage, FaBook, FaCrown, FaTag } from "react-icons/fa6"
import { MdFiberNew } from "react-icons/md"

export type StatusKind = "new" | "read" | "tankoubon" | "patch" | "untagged"

/** `MdFiberNew` carries its own padding, so it is drawn larger to match the others. */
const ICONS: Record<StatusKind, { icon: IconType; size: string }> = {
  new: { icon: MdFiberNew, size: "1.6em" },
  read: { icon: FaCrown, size: "1em" },
  tankoubon: { icon: FaBook, size: "1em" },
  patch: { icon: FaBandage, size: "1em" },
  untagged: { icon: FaTag, size: "1em" },
}

/** Archive status marks. Icons rather than emoji (🆕 👑 📚 🩹 🏷️): CJK fonts ship monochrome glyphs
 *  for some of these that win over the colour emoji font. They inherit the text colour, so every
 *  theme fits. */
export function StatusIcon({ kind, title, style }: { kind: StatusKind; title?: string; style?: CSSProperties }) {
  const { icon: Icon, size } = ICONS[kind]
  return (
    <Icon
      title={title}
      aria-hidden={title ? undefined : true}
      size={size}
      style={{ verticalAlign: "middle", flexShrink: 0, ...style }}
    />
  )
}
