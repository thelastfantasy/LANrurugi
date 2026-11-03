//! 跨语言「通假」归一化 —— 为搜索提供可插拔、可运行时开关的等价折叠。
//!
//! # 设计要点（issue #108 调研）
//!
//! - 所有语言走同一条管线；**索引侧与查询侧必须使用同一个 [`Equivalence`] 实例**
//!   （或至少完全相同的 [`FoldConfig`] 开关）。
//! - `width` 是基线：NFKC 处理全角/半角、兼容字符。但 NFKC 对 U+309B/U+309C 会先
//!   插入空格再拆成 combining mark，所以日文修复放在 NFKC **之后**。
//! - `latin` 去变音时显式排除日文 U+3099/U+309A，避免把 `が/ガ/パ` 退化成
//!   `か/カ/ハ`。
//! - `jp` = 假名 + 日文汉字新旧字体：`wana_kana` 折叠片/平、`ferrous-opencc` 的
//!   `Jp2t` 把 `発/發`、`竜/龍` 这类新/旧字体先收敛；`zh` 再用 OpenCC `T2s` 归一到简体。
//!   两者都在 feature 后面，运行时可分别开关。
//!
//! # 用法
//!
//! ```
//! use lanrurugi_equivalence::{Equivalence, FoldConfig};
//!
//! let eq = Equivalence::new(FoldConfig::new(true, true));
//! let key = eq.fold("ｶ ﾞｷﾞ");
//! assert_eq!(key, "がぎ");
//! ```

mod config;
mod jp;
mod latin;
#[cfg(feature = "opencc")]
mod opencc;
mod pattern;

pub use config::FoldConfig;

use std::borrow::Cow;

use unicode_normalization::UnicodeNormalization;

/// 可复用的通假归一化实例。
///
/// 应用启动时按配置构造一次（通常是 `Arc<Equivalence>`），搜索、索引、OPDS、
/// 分类动态规则全部复用同一实例。`fold_with` / `fold_pattern_with` 允许单次覆盖
/// 语言开关，但调用方必须保证索引也是用同一套开关建立的。
/// 归一化规则 revision。只要规则/依赖行为对 canonical key 有影响，就 +1。
pub const RULE_REVISION: u32 = 1;

pub struct Equivalence {
    config: FoldConfig,
    #[cfg(feature = "opencc")]
    opencc: opencc::OpenCcFolder,
}

impl Equivalence {
    /// 按配置构造实例。未编译进来的语言（feature 未启用）会在运行时被忽略，
    /// 可用 [`Self::languages`] 查询实际生效的开关。
    pub fn new(config: FoldConfig) -> Self {
        #[cfg(not(feature = "zh"))]
        if config.zh {
            tracing::warn!(
                "lanrurugi-equivalence built without `zh` feature; Chinese folding disabled"
            );
        }
        #[cfg(not(feature = "opencc"))]
        if config.jp {
            tracing::debug!(
                "lanrurugi-equivalence built without `opencc` feature; Japanese kanji variants disabled"
            );
        }
        Self {
            config,
            #[cfg(feature = "opencc")]
            opencc: opencc::OpenCcFolder::new(),
        }
    }

    /// 构造时的配置（原样返回，不反映 feature 可用性）。
    pub const fn config(&self) -> FoldConfig {
        self.config
    }

    /// 实际可用的语言开关：未编译进来的语言在此会被置为 `false`。
    pub fn languages(&self) -> FoldConfig {
        #[cfg(all(feature = "zh", feature = "jp"))]
        {
            self.config
        }
        #[cfg(not(all(feature = "zh", feature = "jp")))]
        {
            let mut cfg = self.config;
            #[cfg(not(feature = "zh"))]
            {
                cfg.zh = false;
            }
            #[cfg(not(feature = "jp"))]
            {
                cfg.jp = false;
            }
            cfg
        }
    }

    /// 当前构建是否支持日文汉字新旧字体折叠（依赖 `opencc` feature）。
    pub fn supports_jp_kanji(&self) -> bool {
        cfg!(feature = "opencc")
    }

    /// 预热可选后端（OpenCC 词典）。启动时调用可以避免第一个搜索请求承担词典加载延迟。
    pub fn warmup(&self) {
        #[cfg(feature = "opencc")]
        self.opencc.warmup();
    }

    /// 用实例自身配置折叠一个字符串。无变化时返回 [`Cow::Borrowed`]。
    pub fn fold<'a>(&self, input: &'a str) -> Cow<'a, str> {
        self.fold_with(input, self.config)
    }

    /// 用一次性覆盖配置折叠一个字符串（主要用于测试/诊断）。
    ///
    /// 注意：索引和查询必须用同一套开关，否则会漏匹配——运行时切换语言后需要
    /// 重建索引。
    pub fn fold_with<'a>(&self, input: &'a str, config: FoldConfig) -> Cow<'a, str> {
        if input.is_empty() {
            return Cow::Borrowed(input);
        }

        let mut cur: Cow<'a, str> = Cow::Borrowed(input);

        if config.jp {
            if let Some(next) = jp::pre_normalize(&cur) {
                cur = Cow::Owned(next);
            }
        }
        if config.width {
            if let Some(next) = nfkc(&cur) {
                cur = Cow::Owned(next);
            }
        }
        if config.jp {
            if let Some(next) = jp::post_normalize(&cur) {
                cur = Cow::Owned(next);
            }
        }
        // 日文汉字新旧字体：Jp2t 把新字体/变体收敛到旧字体（如 発→發、竜→龍）。
        // 必须放在 `zh` 的 T2s 之前，这样同时开 zh 时能继续 發→发。
        if config.jp {
            #[cfg(feature = "opencc")]
            if let Some(next) = self.opencc.jp2t(&cur) {
                cur = Cow::Owned(next);
            }
        }
        if config.latin {
            if let Some(next) = latin::fold(&cur) {
                cur = Cow::Owned(next);
            }
        }
        #[cfg(feature = "zh")]
        if config.zh {
            if let Some(next) = self.opencc.t2s(&cur) {
                cur = Cow::Owned(next);
            }
        }
        // feature 关闭时，对应开关只是被忽略；不要留下未使用参数警告。
        let _ = (config.zh, config.jp);

        cur
    }

    /// 折叠搜索 pattern，保留 `*` / `?` 通配符语义。
    pub fn fold_pattern<'a>(&self, pattern: &'a str) -> Cow<'a, str> {
        self.fold_pattern_with(pattern, self.config)
    }

    /// 与 [`Self::fold_pattern`] 相同，但使用一次性覆盖配置。
    pub fn fold_pattern_with<'a>(&self, pattern: &'a str, config: FoldConfig) -> Cow<'a, str> {
        pattern::fold_pattern_with(self, pattern, config)
    }
}

fn nfkc(input: &str) -> Option<String> {
    let out: String = input.nfkc().collect();
    (out != input).then_some(out)
}

impl Default for Equivalence {
    fn default() -> Self {
        Self::new(FoldConfig::all())
    }
}

impl Equivalence {
    /// 索引/查询配置指纹。`FoldConfig` 或规则 revision 变化后必须重建索引。
    pub fn fingerprint(&self) -> String {
        let c = self.languages();
        format!(
            "v{RULE_REVISION}:w={}:zh={}:jp={}:latin={}:crate={}",
            u8::from(c.width),
            u8::from(c.zh),
            u8::from(c.jp),
            u8::from(c.latin),
            env!("CARGO_PKG_VERSION"),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn equivalence_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Equivalence>();
    }

    #[test]
    fn fold_config_partial_json_uses_defaults() {
        let cfg: FoldConfig = serde_json::from_str(r#"{"zh":true,"jp":true}"#).unwrap();
        assert_eq!(
            cfg,
            FoldConfig {
                width: true,
                zh: true,
                jp: true,
                latin: false,
            }
        );
    }
}
