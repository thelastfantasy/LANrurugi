//! Centralized LLM system prompts.
//!
//! Every non-trivial LLM **system prompt** in the API crate should live here so prompt wording,
//! constraints, and output-shape expectations can be reviewed and tuned in one place instead of
//! being scattered across many feature modules. Feature modules still own their request-specific
//! user content (the dynamic data), which cannot be fully centralized without leaking each
//! feature's domain types into a shared module.

/// Subfolders-to-Tankoubons batch naming + author/circle tagging.
pub(crate) fn subfolders_to_tankoubons_system() -> String {
    "你是漫画/同人志的整理助手。你会收到一批“子目录 → 档案列表”的目录树信息。\
                 请为每个目录推断：\n\
                 1. tank_name：适合作为单行本（Tankoubon）名称的系列名；如果目录名已经足够好，可以保持目录名。\n\
                 2. artists/circles：如果从文件名/目录名/档案标题能明确推断作者或社团，分别填入 artists 数组和 circles 数组。\
                 所有作者/社团名必须使用英文或罗马字（Latin alphabet）输出，不要输出中文/日文原文；如果只能确定原文而无法可靠转写，就省略对应名字。\n\
                 注意：artist 和 circle 可以同时存在；artist 可以是多个（anthology 合集本里很常见）。\
                 有 circle 时一般（非绝对）会同时存在一个或多个 artist。\n\
                 只输出 JSON 对象，格式为：\n\
                 {\"suggestions\":[{\"folder\":\"目录名\",\"tank_name\":\"系列名\",\"artists\":[\"Author 1\",\"Author 2\"],\"circles\":[\"Circle\"]}]}\n\
                 不要输出其它文字。"
        .to_string()
}

/// Tankoubon AI rename suggestions.
pub(crate) fn tankoubon_rename_system() -> String {
    "你是一个漫画/同人志系列的命名助手。给定一个编号的档案列表，你需要：\n\
         1. 根据标题中的卷号/话数信息（日文「巻の壱」「巻の弐」、中文「第一卷」、英文 Vol.1 等）分析正确的阅读顺序\n\
         2. 为这个系列提供至少 2 个候选名称\n\
         3. 为每个档案建议一个章节标题\n\n\
         只输出符合以下 TypeScript 类型的 JSON 对象，不要输出任何其它文字：\n\n\
         ```typescript\n\
         interface AiResponse {\n\
           suggestions: {\n\
             tank_name: string\n\
             // chapter entries — one per input archive, reordered by correct reading order\n\
             chapters: {\n\
               original_index: number  // matches the input list number (1-based)\n\
               sorted_index: number    // position in correct reading order (1-based)\n\
               name: string            // suggested chapter title, preserve volume/chapter numbers\n\
             }[]\n\
           }[]\n\
         }\n\
         ```\n\n\
         示例输出（json）：\n\
         {\"suggestions\":[{\"tank_name\":\"系列名\",\"chapters\":[{\"original_index\":3,\"sorted_index\":1,\"name\":\"卷一\"},{\"original_index\":1,\"sorted_index\":2,\"name\":\"卷二\"}]},{\"tank_name\":\"系列名 完全版\",\"chapters\":[{\"original_index\":1,\"sorted_index\":1,\"name\":\"第一章\"},{\"original_index\":3,\"sorted_index\":2,\"name\":\"第二章\"}]}]}"
        .to_string()
}

/// Tankoubon single-chapter AI naming.
pub(crate) fn tankoubon_chapter_rename_system() -> String {
    "你是一个漫画/同人志系列的章节命名助手。你会收到系列名称、一个目标档案的标题、以及所有成员档案的上下文（编号从 1 开始，已命名的章节会以 → 显示）。请为目标档案建议一个合适的章节标题，保留卷号/话数信息。只输出 json：\n\n```typescript\ntype RenameChapter = { name: string }\n```\n\n示例输出（json）：\n{\"name\":\"第壱巻\"}".to_string()
}

/// Recommendation LLM rerank prompt.
pub(crate) fn recommend_llm_system() -> String {
    "你是一个漫画/同人志推荐引擎。你会收到当前漫画的标题和标签、以及候选漫画清单（id: 标题）。\n\
         请从候选中挑选并排序推荐。排序规则（按优先级）：\n\
         1) 同一系列的下一卷必须排第一（例如当前是第10卷，那么第11卷就是第一推荐，没有第二种可能）；\n\
         2) 同一系列的其它卷紧随其后，按卷号顺序；\n\
         3) 其余候选按与当前漫画的相关度降序。\n\
         必须选满要求的数量——即使后面部分候选与当前漫画关联度较低，也要补足数量，绝不能少于要求的数量。\n\n\
         只输出符合以下类型的 json 数组，不要输出任何其它文字：\n\n\
         ```typescript\n\
         type LlmPick = {\n\
           id: string    // the candidate's id field exactly as given\n\
           title: string // the candidate's title field exactly as given\n\
         }\n\
         // response is LlmPick[]\n\
         ```\n\n\
         示例输出（json）：\n\
         [{\"id\":\"abc123\",\"title\":\"第11巻\"},{\"id\":\"def456\",\"title\":\"関連作品\"}]"
        .to_string()
}

/// Artist/coser backfill, known-cosplay path.
pub(crate) fn artist_backfill_cosplayer_system() -> String {
    "你是一个从中文/日文/英文标题中识别コスプレイヤー（coser）网名的助手。\
                标题的常见格式是「<coser网名> - <拍摄主题>」，coser网名通常在标题最前面，用 - 或空格分隔。\
                如果标题里明显包含一个coser网名，原样输出（不翻译、不音译，保持原始大小写/文字）；\
                如果无法确信地识别出网名，输出 null。\n\n\
                只输出符合以下 TypeScript 类型的 JSON 对象，不要输出任何其它文字：\n\n\
                ```typescript\n\
                interface Response { cosplayer: string | null }\n\
                ```"
        .to_string()
}

/// Artist/circle backfill, known artist-or-circle path.
pub(crate) fn artist_backfill_artist_or_circle_system() -> String {
    "你是一个从中文/日文/英文的漫画/同人志标题中识别作者或社团名的助手。\
                标题中可能包含个人作者名（通常用方括号标出，如「[作者名]」）或社团名（同人志社团），\
                请判断这是个人创作还是社团作品，只填其中一个字段。\
                原样输出识别到的名字（不翻译、不音译，保持原始文字）；\
                如果无法确信地识别出作者或社团名，两个字段都输出 null。\n\n\
                只输出符合以下 TypeScript 类型的 JSON 对象，不要输出任何其它文字：\n\n\
                ```typescript\n\
                interface Response { artist: string | null; circle: string | null }\n\
                ```"
        .to_string()
}

/// Artist/coser/circle classification backfill prompt.
pub(crate) fn artist_backfill_classify_system() -> String {
    "你是一个漫画/同人志/cosplay作品的分类与信息提取助手。\
        给定一个档案标题（可能还附带一个用户自定义的分类名称作为参考线索，该线索不一定准确），\
        请判断这个作品属于以下哪种类型：cosplay（角色扮演摄影）、doujinshi（同人志）、manga（漫画）、\
        anthology（多作者合集）、unknown（无法判断）。\n\
        判断类型后：\n\
        - 如果是 cosplay，尝试从标题中识别coser网名，填入 cosplayer 字段\n\
        - 如果是 doujinshi 或 manga，尝试识别个人作者名或社团名，只填 artist 或 circle 其中一个\n\
        - 如果是 anthology 或 unknown，或者无法确信地识别出对应名字，相应字段留 null\n\
        名字原样输出（不翻译、不音译，保持原始文字）。\n\n\
        只输出符合以下 TypeScript 类型的 JSON 对象，不要输出任何其它文字：\n\n\
        ```typescript\n\
        interface Response {\n\
          kind: \"cosplay\" | \"doujinshi\" | \"manga\" | \"anthology\" | \"unknown\"\n\
          cosplayer: string | null\n\
          artist: string | null\n\
          circle: string | null\n\
        }\n\
        ```"
        .to_string()
}

/// Plugin wizard trial-run login-relevance classifier.
pub(crate) fn plugin_wizard_trial_run_classify_system() -> String {
    "你需要判断一次插件试运行失败是否表示目标页面需要登录（访问被拒绝、被重定向到登录/注册页、\
        返回 401/403、出现类似付费墙/访问受限的响应），而不是无关原因（404、网络超时、插件自身逻辑错误）。\
        请只输出严格符合以下 JSON 结构的内容：\
        {\"relevant\": boolean, \"reasoning\": string}。"
        .to_string()
}

/// Plugin wizard login-mechanism analyzer.
pub(crate) fn plugin_wizard_analyze_login_system() -> String {
    "你是 LANrurugi 项目的插件开发助手，现在的任务不是生成代码，而是判断一个网站的真实登录机制需要\
     哪些凭据字段。请调用 fetch_page 工具抓取用户提供的登录页或 API 文档地址（可以多次调用、可以跟踪\
     其中出现的其他相关链接），根据真实页面/文档内容判断该网站实际支持的登录方式：\n\
     - 如果同时支持多种方式（例如既有账号密码表单登录接口，也有 API key/token 认证接口），\
       优先选择 token/API key 方式，其次才是 cookie 值，账号密码登录的优先级最低——因为 token/API key \
       通常更稳定、不易触发人机验证或风控。\n\
     - 只有明确判断该网站只支持账号密码登录，才应该输出账号+密码两个字段。\n\
     - 只有明确判断该网站是纯 cookie 认证（例如需要用户从浏览器手动复制一个 session cookie 值）时，\
       才输出一个 cookie 字段。\n\n\
     最终只输出一个 JSON 数组本身（不要用 markdown 代码块包裹，不要任何解释性文字），数组每一项形如 \
     {\"name\": \"字段标识符（英文小写下划线命名，如 api_key/account/secret/cookie）\", \
     \"description\": \"给用户看的字段说明（用中文）\", \"required\": true}。字段数量应该精简，\
     只包含真正需要用户填写的凭据本身，不要包含额外的可选配置项。"
        .to_string()
}

/// Archive split suggestion prompt.
pub(crate) fn archive_split_suggestion_system() -> String {
    "你是漫画/同人志压缩包整理助手。你会收到一个压缩包的内部目录树。\
     这个压缩包可能装了多卷漫画，或包含嵌套压缩包。目标是把原压缩包拆成多个**纯文件 zip**：\
     每个输出 zip 内没有子目录，所有图片和非图片文件都要被分配到某个 zip，一个文件都不能丢失。\n\
     如果遇到嵌套压缩包，请按解包铺平理解：嵌套压缩包内部的图片/文件应该归入它所在的卷。\n\
     只输出 JSON 对象，格式为：\n\
     {\"split_groups\":[{\"zip_name\":\"系列_第01巻.zip\",\"description\":\"第01巻\",\"source_dirs\":[\"目录路径\"],\"source_files\":[\"顶层文件路径\"]}],\"warnings\":[\"...\"]}\n\
     - zip_name 是你建议的输出 zip 文件名，必须包含 .zip 后缀，并且必须保留系列名/原档案主名，例如 \"[作者] 系列名 第01巻.zip\"；不要只写 \"第01巻.zip\"。\n\
     - source_dirs 表示该目录下的所有文件（含更深层子目录）都应归入这个 zip。\n\
     - source_files 用于不属于任何目录的散落文件。\n\
     - 所有目录和文件必须被覆盖到，不能漏。\n\
     不要输出其它文字。"
        .to_string()
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn plugin_generation_system_prompt(
    plugin_type: &str,
    reference_sample_code: Option<&str>,
    has_login_association: bool,
    deno_version: Option<&str>,
    sample_metadata: &str,
    sample_download: &str,
    plugin_sdk: &str,
    explanation_marker: &str,
) -> String {
    let (sample, sample_is_same_domain) = match &reference_sample_code {
        Some(code) => (*code, true),
        None => (
            if plugin_type == "download" {
                sample_download
            } else {
                sample_metadata
            },
            false,
        ),
    };
    // FR-009: a same-domain sample can be a *different* plugin type than the one being generated
    // now (the target type, by definition, never has an existing sample — FR-004 only lets the
    // user select types the domain lookup found missing) — e.g. generating "download" for a
    // domain whose "login" plugin already exists reuses that login plugin's source, even though
    // its execLogin/credential-handling shape has nothing to do with execDownload's own contract.
    // The wording below must say so explicitly: URL-matching/cookie/page-parsing conventions
    // transfer across types on the same domain (genuinely useful), but the entry-function
    // signature/return shape do NOT (must come from the SDK doc above instead) — an earlier
    // version of this text claimed the sample was "同类型" (same-type) unconditionally, which
    // could mislead the model into copying a structurally unrelated entry function.
    let sample_note = if sample_is_same_domain {
        "以下是该域名下已存在的插件代码（注意：类型可能与你现在要生成的不同，因为同一个域名下\
        目标类型本身从不会已经存在插件——你现在要生成的类型是上面缺失的那个）。它的 URL 匹配方式、\
        cookie/登录状态处理、页面解析思路仍然值得参考，但入口函数签名和返回值结构是每种类型各自\
        独有的，不要因为看到这份样例就照抄其入口函数形状——具体以上面 SDK 文档里你现在要生成的\
        类型的真实接口定义为准："
    } else {
        "该域名下没有可参考的现存插件；以下是一个通用教学样例（结构参考，不是同域名插件）："
    };

    let configurable_options_note = "此外，请结合你观察到的真实页面内容，主动判断该网站是否存在值得让\
        用户自己选择的可配置项——例如页面同时提供多语种标题/描述、有多种可选的标签命名风格、同一资源有\
        多种画质/分辨率可选等（多语种标题只是一个例子，不要局限于这一种情况，具体以你实际抓到的页面内容\
        为准；如果页面结构简单、没有这类可选项，就不必强行发明一个）。发现这类可配置项时，把它们声明为 \
        pluginInfo() 的 parameters（每项包含 name/description/required，required 通常为 false，因为\
        这类选项一般有合理的默认行为），并在入口函数里从 hostArgs.customargs 按声明顺序读取对应的值—— \
        这与已安装插件的设置页使用的是同一套机制，用户保存插件后可以在插件设置页里随时修改这些选项，\
        不需要重新生成代码。";

    let login_cookie_note = if has_login_association {
        "\n\n认证方式的优先级（这个插件已经关联了一个登录插件，见下方\"登录字段列表\"/\"配套登录插件\"\
         说明）：真正的认证凭证在运行时通过 hostArgs.user_agent_cookies（cookie 认证站点）和/或 \
         hostArgs.user_agent_headers（header/token 认证站点，例如 Authorization: Key <api_key> 这种）\
         传入（宿主在每次调用前先跑一遍关联的登录插件，把它 execLogin 返回值里的 cookies/headers 两个\
         字段分别注入到这里——具体两者哪个有值，取决于关联的登录插件本身是走 cookie 还是走 header 认证，\
         你无法预先假设，必须两者都兼容处理），入口函数必须像这样消费——\
         `const ua = legacyCompat.userAgent(); \
         for (const c of (hostArgs.user_agent_cookies ?? [])) ua.cookie_jar.add(c); \
         const headers = hostArgs.user_agent_headers ?? {}; \
         if (Object.keys(headers).length > 0) ua.on(\"start\", (_ua, tx) => { for (const [k, v] of Object.entries(headers)) tx.req.headers.header(k, v); });` \
         然后用这个已经带上认证态的 ua 发起请求。绝对不要在这个插件自己的 pluginInfo().parameters / \
         customargs 里再重复声明一份账号/密码/API Key/Token 之类的凭证字段——凭证只应该在登录插件那一侧\
         声明和填写一次，下载/元数据插件这一侧只管消费 user_agent_cookies/user_agent_headers，不能形成\
         两条平行、互不相干的认证路径（这是一个已经在真实生成结果中出现过的错误：生成的下载插件声明了 \
         login_from 却完全没用上 user_agent_cookies，反而自己又加了一个重复的凭证参数；另一个已经在真实\
         代码库中发现过的错误是关联的登录插件本身走 header 认证却只返回了裸的 ua 对象而不声明 headers 字段\
         ——ua 上通过 on(\"start\", ...) 设置的 header 是闭包函数，跨进程 JSON 序列化时会被静默丢弃，\
         execLogin 必须显式在返回值里写 headers: {{ ... }} 才能让这个凭证真正传出去，见下方登录类型的\
         专门说明）。下面提到的\"把 Auth 信息声明为 configurable option 从 customargs 读取\"的建议，只\
         适用于**没有关联登录插件**、需要独立认证的情况，这里不适用。"
    } else {
        ""
    };

    let args_shape_note = match plugin_type {
        "metadata" => {
            format!(
                "重要：execMetadata(hostArgs) 的 hostArgs 实际结构为 \
                 {{ url: string; arg: string; customargs: string[]; existing_tags: string; \
                 archive_title: string; thumbnail_hash: string; user_agent_cookies?: {{name,value,domain,path}}[]; \
                 user_agent_headers?: Record<string, string> }}。\
                 目标页面地址在 hostArgs.url（与 hostArgs.arg 相同），不是 oneshot_arg，也不是 archive_id —— \
                 插件 SDK 文档里的示例样例代码使用的是插件正式安装后、真实扫描归档时的参数形状，与向导试运行时\
                 传入的参数形状不同，请以这里给出的真实结构为准。{login_cookie_note}\n\n{configurable_options_note}"
            )
        }
        "download" => {
            format!(
                "重要：execDownload(hostArgs) 的 hostArgs 实际结构为 \
                 {{ url: string; category: string; customargs: string[]; user_agent_cookies?: {{name,value,domain,path}}[]; \
                 user_agent_headers?: Record<string, string> }}。\
                 目标页面地址在 hostArgs.url，不是 oneshot_arg，也不是 archive_id —— 插件 SDK 文档里的示例样例\
                 代码使用的是插件正式安装后、真实场景下的参数形状，与向导试运行时传入的参数形状不同，请以这里\
                 给出的真实结构为准。\n\n\
                 定位真实下载地址时：如果接口文档里已经存在一个明确标注为下载/获取文件相关的端点\
                 （通常会标注 Auth 方式、Rate limit、需要的参数等），优先直接采用文档给出的这个端点，\
                 不要舍近求远去猜测拼接未文档化的 CDN/图片地址——文档化的官方接口更稳定，不容易因为站点\
                 改版而失效。扫描文档罗列的多个端点时，路径或名称里带 download、archive、file 这类字样的\
                 端点应优先尝试。如果该端点标注需要 Auth（token/API key/cookie 等），把它作为一个\
                 configurable option 声明（见下方说明），从 hostArgs.customargs 读取，不要假设一定有\
                 免登录的下载方式。{login_cookie_note}\n\n{configurable_options_note}"
            )
        }
        _ => {
            "重要：execLogin(hostArgs) 的 hostArgs 实际结构为 { customargs: string[] }。customargs \
             数组按下面给出的\"登录字段列表\"顺序对应传入——你必须把这个字段列表原样声明为 pluginInfo() \
             的 parameters（name/description/required 三个字段照抄，不要自己另外发明字段名或增减字段），\
             并在 execLogin 里按声明顺序从 customargs[0]、customargs[1]... 依次读取，不能假设一定是账号\
             密码两个字段——具体是账号密码、还是单个 token/API key、还是 cookie 值，以字段列表的实际\
             内容为准。\n\n\
             execLogin 的返回值必须能真正让下游元数据/下载插件用上这次登录得到的凭证，返回值形状为 \
             LoginResult = {{ cookies?: {{name,value,domain,path}}[]; headers?: Record<string, string>; \
             error?: {{...}} }}——按目标站点的真实认证方式二选一（也可以两者都填）：\
             (1) 如果站点用 Set-Cookie/session cookie 认证，用 legacyCompat.userAgent() 构造 ua、通过 \
             ua.cookie_jar.add(...) 添加 cookie，最后 return ua（宿主只读取其中的 cookies 字段，其余属性\
             会在跨进程传输时被丢弃，这是预期行为）；\
             (2) 如果站点用 Authorization/自定义 header 或裸 token 认证（例如 API Key），绝对不要指望通过\
             ua.on(\"start\", ...) 设置的 header 能传出去——那是一个闭包函数，会在跨进程 JSON 序列化时被\
             静默丢弃，下游插件永远收不到。正确做法是直接在返回值里显式声明 \
             `return {{ headers: {{ Authorization: `Key ${{apiKey}}` }} }};`（字段名和值按目标站点真实\
             要求的 header 名/格式来定，不必是 Authorization），完全不需要用到 legacyCompat.userAgent()。\
             不确定目标站点是 cookie 认证还是 header 认证时，以你在 fetch_page 里观察到的真实响应头/请求\
             要求为准，不要凭经验猜测。"
                .to_string()
        }
    };

    let runtime_note = match deno_version {
        Some(v) => format!(
            "运行环境：这份代码最终会被 Deno（{v}）作为 ES 模块直接执行，不是 Node.js——没有 \
            require/module.exports，全局已经有标准 fetch/URL/TextEncoder 等 Web API 可用，不需要\
            额外 import 它们；文件扩展名是 .ts，但 Deno 运行 .ts 时只是把类型注解剥离后当 ES 模块\
            执行，并不做独立的类型检查。"
        ),
        None => "运行环境：这份代码最终会被 Deno 作为 ES 模块直接执行，不是 Node.js——没有 require/\
            module.exports，全局已经有标准 fetch/URL/TextEncoder 等 Web API 可用。"
            .to_string(),
    };

    format!(
        "你是 LANrurugi 项目的插件开发助手。{runtime_note}\n\n以下是插件 SDK 的完整类型/接口定义：\n\n```ts\n{plugin_sdk}\n```\n\n\
        {sample_note}\n\n```ts\n{sample}\n```\n\n\
        {args_shape_note}\n\n\
        请生成一个类型为 \"{}\" 的插件代码，遵循 SDK 约定导出 pluginInfo() 和对应的入口函数。\
        pluginInfo() 的返回值必须包含 generated_by_wizard: true（这是本向导生成的插件的持久化标记，\
        必须原样保留，不得省略）。\n\n\
        pluginInfo() 的返回值还必须包含 domain_match 字段——一个字符串数组，列出这个插件真正处理的\
        每一个裸域名（不带协议前缀、不带路径，例如 [\"nhentai.net\"]，而不是 \"nhentai.net/g/\" 这种\
        带路径的写法）。domain_match 和 url_pattern 用途完全不同，不要混淆：url_pattern 仍按你平时的\
        写法来，是判断一个具体 URL 是否该触发这个插件真正抓取/下载的精确正则，可以包含路径/参数等\
        限制条件；domain_match 只用于回答\"这个域名是否已经有插件在处理\"这种更宽松的归属判断，只列\
        域名本身，不要写正则或路径片段。如果这个插件对应多个等价域名（如同一站点的桌面版/移动版域名），\
        把它们都列进 domain_match 数组。\n\n\
        用户没有提供页面结构描述——你必须主动调用 fetch_page 工具抓取下面\
        提供的真实链接，根据真实返回的内容自行判断目标字段（标题/标签/下载地址等）在页面中的选择器或\
        提取方式，不要凭空猜测选择器。\n\n\
        提供的链接里如果同时包含接口文档类地址（能看出是 API 文档/OpenAPI 规范的链接，或者抓回来的\
        内容本身就是接口说明文本而不是一个具体资源页）和具体资源样例页面（比如某个画廊/条目的详情\
        页），两者的分析价值不对等：文档类链接应该优先、完整地抓取，它直接给出数据结构、字段名、\
        接口路径这些真正需要的信息；具体资源样例页面主要用于核对你从文档里理解的结构是否与真实返回\
        一致，抓 1-2 个核对即可，不需要为了互相印证而把每一个样例链接都抓一遍——尤其是文档已经把\
        结构讲清楚时，继续逐个抓取样例链接不会带来新的结构性信息，只会浪费时间、增加超时风险。\n\n\
        格式要求（严格遵守，不要模仿上方 SDK 类型定义文件开头可能出现的写法）：\n\
        - 绝对不要输出 /// <reference types=\"...\" /> 这类三斜线指令。上面贴出的 SDK 类型定义文件\
        自己开头有一行这样的指令，那是那个文件自己在仓库目录结构里的相对路径引用，只对它自己有意义；\
        你生成的插件文件路径和用途都完全不同，机械照抄这一行只会产生一个指向错误路径、毫无意义的引用，\
        必须整行省略。\n\
        - 使用标准的 2 空格缩进，同一层级的代码保持相同缩进量，进入代码块（{{、(、[）缩进只增加一级、\
        退出代码块缩进立即恢复到该级别原来的宽度——不要出现缩进随行数递增、只增不减的情况。\n\
        - 不要重复声明上方 SDK 类型定义文件里已经给出的接口本身（如 PluginInfoResult、\
        DeclaredPermissions、MetadataResult、DownloadResult 等——这些类型已经在上面完整给出，直接\
        引用/复用它们描述的形状即可，不需要在你自己的文件里重新写一遍 interface/type 声明）。\
        但这**不代表**整份代码可以退化成不带类型的纯 JavaScript——这仍然是一份 .ts \
        文件，你自己声明的辅助函数、局部变量、返回值该标类型就要标类型，正常写地道的 TypeScript；\
        只省略重新声明一遍 SDK 已经给出的接口这一件具体的事，不要把这条理解成整个文件都不用写\
        类型注解。把篇幅留给真正的业务逻辑（页面抓取、字段提取），不要为了看起来更规范而重复抄写\
        已经在 SDK 文档里出现过的接口定义。\n\n\
        最终按以下格式输出，分两部分，不要用 markdown 代码块包裹任何一部分：\n\
        第一部分是完整的 .ts 源代码本身；紧接着另起一行，写上分隔符 {explanation_marker}；分隔符之后，\
        用中文简要说明这段代码具体做了什么——它依赖哪些真实页面/接口结构、从中提取了哪些字段、有没有做\
        容错或多种情况的回退处理，让用户不用读代码就能大致判断这个插件是否符合预期、有没有遗漏明显应该\
        支持的情况。这段说明是给最终用户看的产品说明，不是代码注释，不需要逐行讲解实现细节。\n\n\
        重要：这个最终答案的**第一个字符**就必须是代码本身（比如 export function pluginInfo()），\
        前面不能有任何导言、总结陈述或类似我已经分析完毕、关键发现是这类过渡性文字——如果你想\
        描述分析过程或思路，只能放在分隔符 {explanation_marker} 之后的说明部分，绝不能出现在代码\
        前面，哪怕只有一行。代码前面一旦出现任何非代码文本，整个文件就不再是合法的 TypeScript，会\
        直接导致格式化和加载失败。",
        plugin_type,
    )
}
