/*
 * Trilingual copy. Chinese is the source language (the project's own docs and CLAUDE.md are
 * Chinese), English and Japanese are full translations rather than subsets — the language switcher
 * never leaves a half-translated page behind.
 */
const COPY = {
  zh: {
    "ai.title": "AI 功能",
    "ai.lead":
      "不是零散的玩具功能：几个过去只能手工完成的场景被做成了完整闭环，而且全部可选 —— 不配 LLM Key 也能优雅降级。",
    "ai.wizard": "AI 插件创建向导",
    "ai.wizardBody":
      "用自然语言描述目标站点 → 自动生成 .ts 插件 → Deno 沙箱真实试运行 → 按报错自动修复 → 确认安装。页面抓取由系统执行，测试凭据永远不会发给 AI。",
    "ai.recommend": "智能推荐",
    "ai.recommendBody":
      "阅读到卷尾时给出推荐卡片：本地 ONNX 嵌入模型粗筛，再由 LLM 精排；单行本以其最后一卷为锚点计算。",
    "ai.translate": "漫画翻译 / OCR",
    "ai.translateBody":
      "逐页 OCR、字形掩膜擦除与译文排版，支持本地后端与云端后端；阅读页可随时开关。",
    "ai.tankoubon": "AI 单行本整理",
    "ai.tankoubonBody":
      "从标题识别卷号与话数，建议单行本名称、章节标题和阅读顺序；也能把散档案自动归并成系列。",
    "html.lang": "zh-CN",
    "doc.title": "LANrurugi — 自托管漫画 / 同人志库管理器",
    "doc.description":
      "LANrurugi — 用 Rust 与 React 重写的 LANraragi，自托管漫画/同人志库管理器，与现有 LANraragi 部署保持数据与 API 兼容。",
    eyebrow: "自托管漫画 / 同人志库管理器",
    tagline:
      "LANraragi 的 Rust + React 重写版。继续使用你现有的 Redis 数据、档案目录和第三方客户端，界面与性能换成一套新的。",
    "feature.compat":
      "沿用 LANraragi 的 Redis 键、目录结构与 HTTP API，可直接替换现有部署",
    "feature.ingest": "不合并、不改名的入库流程，保留原始压缩包与文件名",
    "feature.search":
      "命名空间检索（artist / circle / cosplayer / language……）与服务端搜索历史",
    "feature.api": "Casbin 逐路由授权，242 个接口全部纳入管理，文档由源码生成",
    "feature.clients": "支持第三方阅读器、订阅、翻译与 AI 插件",
    "link.github": "GitHub 仓库",
    "link.docs": "文档",
    "link.api": "API 参考",
    "link.sdk": "插件 SDK",
    note: "需要自托管：一台能跑 Docker 的机器，加上你现有的 LANraragi 数据即可开始。",
  },
  en: {
    "ai.title": "AI features",
    "ai.lead":
      "Not scattered toys: a few things that used to be done by hand are complete workflows, and every one of them is optional — without an LLM key they degrade gracefully.",
    "ai.wizard": "AI plugin wizard",
    "ai.wizardBody":
      "Describe the site in plain language → generate a .ts plugin → run it for real in a Deno sandbox → let the AI repair it from the error → confirm install. The system does every fetch; test credentials are never sent to the AI.",
    "ai.recommend": "Smart recommendations",
    "ai.recommendBody":
      "At the end of a volume the reader offers recommendation cards: a local ONNX embedding model shortlists, an LLM re-ranks, and a tankoubon is anchored on its last volume.",
    "ai.translate": "Manga translation / OCR",
    "ai.translateBody":
      "Per-page OCR, glyph-mask erasing and typesetting, with local and cloud backends — switchable in the reader at any time.",
    "ai.tankoubon": "AI tankoubon curation",
    "ai.tankoubonBody":
      "Reads volume and chapter numbers out of titles, then suggests the tankoubon name, chapter titles and reading order — and groups loose archives into series.",
    "html.lang": "en",
    "doc.title": "LANrurugi — self-hosted manga / doujinshi library manager",
    "doc.description":
      "LANrurugi is a Rust and React rewrite of LANraragi: a self-hosted manga and doujinshi library manager that stays compatible with existing LANraragi data and HTTP APIs.",
    eyebrow: "Self-hosted manga / doujinshi library manager",
    tagline:
      "A Rust + React rewrite of LANraragi. Keep your existing Redis data, archive directories and third-party clients; get a new interface and a new engine.",
    "feature.compat":
      "Reuses LANraragi's Redis keys, directory layout and HTTP API — drop it in place of an existing deployment",
    "feature.ingest":
      "Non-merging, non-renaming ingestion: original archives and filenames are preserved",
    "feature.search":
      "Namespace searches (artist / circle / cosplayer / language …) with server-side search history",
    "feature.api":
      "Casbin authorization on every route — all 242 operations covered, documentation generated from source",
    "feature.clients":
      "Works with third-party readers, subscriptions, translation and AI plugins",
    "link.github": "GitHub repository",
    "link.docs": "Documentation",
    "link.api": "API reference",
    "link.sdk": "Plugin SDK",
    note: "Self-hosted: a machine that can run Docker, plus your existing LANraragi data.",
  },
  ja: {
    "ai.title": "AI 機能",
    "ai.lead":
      "散発的なおもちゃではありません。かつて手作業だった場面を一連の流れにしたもので、しかもすべて任意です — LLM キーがなくても丁寧に縮退します。",
    "ai.wizard": "AI プラグイン作成ウィザード",
    "ai.wizardBody":
      "対象サイトを自然言語で説明 → .ts プラグインを生成 → Deno サンドボックスで実際に試走 → エラーから自動修正 → 確認してインストール。取得はすべてシステムが行い、テスト用の資格情報が AI に渡ることはありません。",
    "ai.recommend": "スマート推薦",
    "ai.recommendBody":
      "巻の終わりに推薦カードを表示します。ローカルの ONNX 埋め込みで絞り込み、LLM が並べ替え、単行本は最終巻を基準に計算します。",
    "ai.translate": "漫画翻訳 / OCR",
    "ai.translateBody":
      "ページ単位の OCR、字形マスクの消去、訳文の組み付け。ローカル／クラウド両方のバックエンドに対応し、ビューアからいつでも切り替えられます。",
    "ai.tankoubon": "AI 単行本整理",
    "ai.tankoubonBody":
      "タイトルから巻数・話数を読み取り、単行本名、章タイトル、読む順を提案します。バラのアーカイブをシリーズにまとめることもできます。",
    "html.lang": "ja",
    "doc.title": "LANrurugi — セルフホスト型 漫画・同人誌ライブラリ管理",
    "doc.description":
      "LANrurugi は LANraragi を Rust と React で書き直した、セルフホスト型の漫画・同人誌ライブラリ管理ツールです。既存の LANraragi のデータと HTTP API と互換性があります。",
    eyebrow: "セルフホスト型 漫画・同人誌ライブラリ管理",
    tagline:
      "LANraragi の Rust + React 書き直し版。既存の Redis データ、アーカイブのディレクトリ、サードパーティ製クライアントをそのまま使いながら、UI とエンジンを新しくできます。",
    "feature.compat":
      "LANraragi の Redis キー、ディレクトリ構成、HTTP API をそのまま利用 — 既存の環境と置き換え可能",
    "feature.ingest":
      "統合も改名もしない取り込み — 元のアーカイブとファイル名を保持します",
    "feature.search":
      "名前空間つき検索（artist / circle / cosplayer / language など）とサーバー側の検索履歴",
    "feature.api":
      "Casbin によるルート単位の認可 — 242 個の操作をすべて管理し、ドキュメントはソースから生成",
    "feature.clients":
      "サードパーティ製ビューア、購読、翻訳、AI プラグインに対応",
    "link.github": "GitHub リポジトリ",
    "link.docs": "ドキュメント",
    "link.api": "API リファレンス",
    "link.sdk": "プラグイン SDK",
    note: "セルフホスト前提：Docker が動くマシンと、既存の LANraragi データがあれば始められます。",
  },
};

const STORAGE_KEY = "lanrurugi-home-lang";
const SUPPORTED = Object.keys(COPY);

/** `?lang=ja` wins, then the last explicit choice, then the browser's own preference. */
function initialLanguage() {
  const fromQuery = new URLSearchParams(location.search).get("lang");
  if (fromQuery && SUPPORTED.includes(fromQuery)) return fromQuery;
  try {
    const stored = localStorage.getItem(STORAGE_KEY);
    if (stored && SUPPORTED.includes(stored)) return stored;
  } catch {
    /* storage can be unavailable (private mode, blocked cookies) — fall through */
  }
  const preferred = (navigator.languages || [navigator.language || "zh"]).find((tag) =>
    SUPPORTED.some((lang) => tag.toLowerCase().startsWith(lang)),
  );
  return preferred ? preferred.slice(0, 2).toLowerCase() : "zh";
}

function apply(lang) {
  const copy = COPY[lang];
  document.documentElement.lang = copy["html.lang"];
  document.title = copy["doc.title"];
  document
    .querySelector('meta[name="description"]')
    ?.setAttribute("content", copy["doc.description"]);
  for (const node of document.querySelectorAll("[data-i18n]")) {
    const value = copy[node.dataset.i18n];
    if (value !== undefined) node.textContent = value;
  }
  for (const button of document.querySelectorAll(".langs button")) {
    button.setAttribute("aria-pressed", String(button.dataset.lang === lang));
  }
}

function choose(lang) {
  apply(lang);
  try {
    localStorage.setItem(STORAGE_KEY, lang);
  } catch {
    /* see initialLanguage */
  }
  const url = new URL(location.href);
  url.searchParams.set("lang", lang);
  history.replaceState(null, "", url);
}

for (const button of document.querySelectorAll(".langs button")) {
  button.addEventListener("click", () => choose(button.dataset.lang));
}

apply(initialLanguage());
