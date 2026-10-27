/**
 * The share card's own strings. Kept here rather than in the library's locale files so the card
 * does not load them; mirror any change in `share.card.*`/`share.page.*` there.
 */
export interface CardText {
  unavailable: string
  restricted: string
  restrictedBody: string
  open: string
  read: string
  download: string
  close: string
  importHint: string
  importHintShort: string
  copyCode: string
  unavailableItem: string
  items: (count: number) => string
  volumes: (count: number) => string
  more: (count: number) => string
  viewList: string
  viewIcons: string
  viewThumbs: string
  /** A share that can no longer be opened, by why. */
  closed: Record<"expired" | "revoked" | "exhausted", { title: string; body: string }>
}

const TEXT: Record<"zh" | "en" | "ja", CardText> = {
  zh: {
    unavailable: "该分享已不可用。",
    restricted: "受限分享",
    restrictedBody: "该分享仅限特定地址访问。",
    open: "打开",
    read: "阅读",
    download: "下载",
    close: "关闭",
    importHint: "在你自己的 LANrurugi 中导入：上传 → 导入分享。",
    importHintShort: "上传 → 导入分享",
    copyCode: "复制分享码",
    unavailableItem: "已不可用",
    items: (n) => `共 ${n} 项`,
    volumes: (n) => `${n} 卷`,
    more: (n) => `还有 ${n} 项`,
    viewList: "列表",
    viewIcons: "图标",
    viewThumbs: "缩略图",
    closed: {
      expired: { title: "分享已过期", body: "有效期已过，内容已无法查看或下载。" },
      revoked: { title: "分享已被撤销", body: "分享方已撤销这个分享。" },
      exhausted: { title: "下载次数已达上限", body: "设置的下载次数已经用完，内容已无法查看或下载。" },
    },
  },
  en: {
    unavailable: "This share is no longer available.",
    restricted: "Restricted share",
    restrictedBody: "This share is limited to certain addresses.",
    open: "Open",
    read: "Read",
    download: "Download",
    close: "Close",
    importHint: "Import it in your own LANrurugi: Upload → Import share.",
    importHintShort: "Upload → Import share",
    copyCode: "Copy code",
    unavailableItem: "No longer available",
    items: (n) => `Items: ${n}`,
    volumes: (n) => `${n} volumes`,
    more: (n) => `${n} more`,
    viewList: "List",
    viewIcons: "Icons",
    viewThumbs: "Thumbnails",
    closed: {
      expired: { title: "This share has expired", body: "It can no longer be viewed or downloaded." },
      revoked: { title: "This share was revoked", body: "The sharer revoked it." },
      exhausted: { title: "Download limit reached", body: "Its downloads are used up; it can no longer be viewed or downloaded." },
    },
  },
  ja: {
    unavailable: "この共有は利用できなくなりました。",
    restricted: "制限付きの共有",
    restrictedBody: "この共有は特定のアドレスに限定されています。",
    open: "開く",
    read: "読む",
    download: "ダウンロード",
    close: "閉じる",
    importHint: "自分の LANrurugi でインポート：アップロード → 共有をインポート。",
    importHintShort: "アップロード → 共有をインポート",
    copyCode: "コードをコピー",
    unavailableItem: "利用できなくなりました",
    items: (n) => `${n} 件`,
    volumes: (n) => `${n} 巻`,
    more: (n) => `ほか ${n} 件`,
    viewList: "リスト",
    viewIcons: "アイコン",
    viewThumbs: "サムネイル",
    closed: {
      expired: { title: "共有の期限が切れました", body: "閲覧もダウンロードもできません。" },
      revoked: { title: "共有は取り消されました", body: "共有元がこの共有を取り消しました。" },
      exhausted: { title: "ダウンロード回数の上限に達しました", body: "ダウンロード回数を使い切ったため、閲覧もダウンロードもできません。" },
    },
  },
}

/** `?lang=` from the embedding page, else the visitor's browser languages, else English. */
export function cardText(): CardText {
  const asked = new URLSearchParams(window.location.search).get("lang")
  for (const tag of [asked, ...navigator.languages]) {
    const base = tag?.toLowerCase().split("-")[0]
    if (base === "zh" || base === "en" || base === "ja") return TEXT[base]
  }
  return TEXT.en
}
