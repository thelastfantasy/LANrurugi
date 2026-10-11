//! OpenCC 后端（`ferrous-opencc`，纯 Rust、Apache-2.0）。
//!
//! 两个独立转换：
//! - `t2s`：中文繁 -> 简，索引/查询都归一到简体；
//! - `jp2t`：日文新字体 -> 旧字体/繁体，把 `発/發`、`竜/龍` 这类日文新旧字体先收敛，
//!   如果同时开了 `zh`，后续 `t2s` 会继续把旧字体收敛到简体（`発`→`發`→`发`）。
//!
//! 两个词典都懒加载，`warmup()` 可在启动时预热。

use std::sync::OnceLock;

use ferrous_opencc::{config::BuiltinConfig, OpenCC};

pub(crate) struct OpenCcFolder {
    t2s: OnceLock<Option<OpenCC>>,
    jp2t: OnceLock<Option<OpenCC>>,
}

impl OpenCcFolder {
    pub(crate) fn new() -> Self {
        Self {
            t2s: OnceLock::new(),
            jp2t: OnceLock::new(),
        }
    }

    pub(crate) fn warmup(&self) {
        let _ = converter(&self.t2s, BuiltinConfig::T2s, "T2s");
        let _ = converter(&self.jp2t, BuiltinConfig::Jp2t, "Jp2t");
    }

    /// 中文繁 -> 简。
    pub(crate) fn t2s(&self, input: &str) -> Option<String> {
        let cc = converter(&self.t2s, BuiltinConfig::T2s, "T2s")?;
        let out = cc.convert(input);
        (out != input).then_some(out)
    }

    /// 日文新字体 -> 旧字体/繁体。
    pub(crate) fn jp2t(&self, input: &str) -> Option<String> {
        let cc = converter(&self.jp2t, BuiltinConfig::Jp2t, "Jp2t")?;
        let out = cc.convert(input);
        (out != input).then_some(out)
    }
}

fn converter<'a>(
    cell: &'a OnceLock<Option<OpenCC>>,
    config: BuiltinConfig,
    label: &'static str,
) -> Option<&'a OpenCC> {
    cell.get_or_init(|| match OpenCC::from_config(config) {
        Ok(cc) => Some(cc),
        Err(err) => {
            tracing::error!(%err, label, "failed to load OpenCC dictionary; conversion disabled");
            None
        }
    })
    .as_ref()
}
