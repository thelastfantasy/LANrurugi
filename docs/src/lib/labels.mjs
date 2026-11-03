// UI strings for the generated API pages, per locale.
//
// Kept apart from the components so every reader-facing word on these pages comes from one file —
// the generated pages themselves contain only headings and document prose, and the chrome around
// them is translated here. `zh` is the site's default locale; anything missing falls back to `en`.

const LABELS = {
  zh: {
    queryParameters: '查询参数',
    pathParameters: '路径参数',
    requestBody: '请求体',
    headerParameters: '请求头',
    responses: '响应',
    required: '必填',
    optional: '可选',
    response: '响应',
    server: '服务器地址',
    serverHint: '填写你自己的实例地址，请求由浏览器直接发往该地址。',
    token: 'API 令牌',
    tokenHint: '在实例的设置页面创建；只保存在本机浏览器里。',
    send: '发送',
    sending: '发送中…',
    copyCurl: '复制为 cURL',
    copied: '已复制',
    noExample: '该响应没有示例数据。',
    noCredentials: '该端点不需要认证。',
    accessNotes: '权限差异',
    sessionOnly:
      '该端点**只接受网页会话**（登录本站所用实例后浏览器自动携带的 cookie）。API 令牌会被服务端拒绝，因此这里不提供令牌输入框，上面的命令行也不带 Authorization 头。',
    tokenOnly: '该端点只接受 API 令牌，不接受网页会话。',
    adminTokenOnly:
      '该端点需要**管理员**令牌：访客（只读）令牌会被服务端拒绝。',
    failed: '请求失败',
    failedHint:
      '常见原因：实例地址填错或不可达；实例未开启「为客户端 API 启用 CORS」；或者文档站是 HTTPS 而实例是局域网内的纯 HTTP（浏览器按混合内容拦截）。可以用上面的「复制为 cURL」在终端里发同样的请求。',
    status: '状态',
  },
  en: {
    queryParameters: 'Query parameters',
    pathParameters: 'Path parameters',
    requestBody: 'Request body',
    headerParameters: 'Headers',
    responses: 'Responses',
    required: 'required',
    optional: 'optional',
    response: 'Response',
    server: 'Server',
    serverHint: 'Your own instance address. Requests go from this browser straight to it.',
    token: 'API token',
    tokenHint: 'Create one in your instance’s settings. Stored in this browser only.',
    send: 'Send',
    sending: 'Sending…',
    copyCurl: 'Copy as cURL',
    copied: 'Copied',
    noExample: 'This response carries no example body.',
    noCredentials: 'This endpoint needs no credentials.',
    accessNotes: 'How access changes the result',
    sessionOnly:
      'This endpoint accepts a **browser session only** — the cookie set when you log in to your own instance. API tokens are refused by the server, so there is no token field here and the command above carries no Authorization header.',
    tokenOnly: 'This endpoint accepts an API token, not a browser session.',
    adminTokenOnly:
      'This endpoint needs an **administrator** token — a guest (read-only) token is refused by the server.',
    failed: 'Request failed',
    failedHint:
      'Usual causes: the instance address is wrong or unreachable; cross-origin access is not enabled on the instance; or this page is HTTPS while the instance is plain HTTP on a LAN address, which the browser blocks as mixed content. “Copy as cURL” above sends the same request from a terminal.',
    status: 'Status',
  },
}

/**
 * Resolve a locale to this file's keys.
 *
 * Starlight hands components `Astro.currentLocale`, which is the *language tag* of the active
 * locale (`zh-CN`) or the literal `'root'` for the default one — not the short keys used here. The
 * first build compared them directly and so silently fell back to English on every Chinese page
 * (`Send` and `Query parameters` on a page whose `<html lang>` was `zh-CN`). `root` maps to Chinese
 * because that is what this site's default locale is.
 */
export function labels(locale) {
  const raw = String(locale ?? '').toLowerCase()
  const key = raw === 'root' || raw === '' ? 'zh' : raw.split('-')[0]
  return LABELS[key] ?? LABELS.en
}
