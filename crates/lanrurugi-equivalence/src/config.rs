use serde::{Deserialize, Serialize};

/// 搜索「通假」的语言开关。
///
/// `width` 是基线 Unicode 兼容归一化（NFKC），默认开启；其余三个是可选语言折叠。
/// `serde` 反序列化缺省字段时使用 [`FoldConfig::default`]，所以
/// `{ "zh": true, "jp": true }` 会得到 `width: true, latin: false`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FoldConfig {
    /// 基线 Unicode NFKC：全角/半角、兼容字符。默认 `true`。
    pub width: bool,
    /// 中文繁简互通（OpenCC，归一到简体）。需要 `zh` feature。
    pub zh: bool,
    /// 日文假名互通：半角、浊点/半浊点分离、片假名⇄平假名。需要 `jp` feature。
    pub jp: bool,
    /// 拉丁变音符号折叠（`MÄR` ≡ `MAR`）。
    pub latin: bool,
}

impl Default for FoldConfig {
    fn default() -> Self {
        Self {
            width: true,
            zh: false,
            jp: false,
            latin: false,
        }
    }
}

impl FoldConfig {
    /// `{ zh, jp }` 的便捷构造，其余保持默认（`width: true, latin: false`）。
    pub const fn new(zh: bool, jp: bool) -> Self {
        Self {
            width: true,
            zh,
            jp,
            latin: false,
        }
    }

    /// 全部语言开启。
    pub const fn all() -> Self {
        Self {
            width: true,
            zh: true,
            jp: true,
            latin: true,
        }
    }
}
