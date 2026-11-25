//! 日文折叠：半角/浊点分离修复 + 片假名→平假名。
//!
//! 现成 crate 实测（issue #108 补充调研）没有哪个能覆盖「假名和浊点之间夹空格」，
//! 且 NFKC 对 U+309B/U+309C 会先插入空格再拆成 combining mark，导致之后无法合成。
//! 因此日文步骤放在 **NFKC 之后**：先把 `kana + [可选空白] + mark` 修复成可合成的
//! 形式，再交给 NFC/`wana_kana`。

#[cfg(feature = "jp")]
use wana_kana::{ConvertJapanese, Options};

/// NFKC 之前：把日文波浪线统一成长音符号 `ー`。NFKC 会把 `～`(U+FF5E) 变成
/// ASCII `~`，之后 `keep_prolonged` 就救不回来了。
pub(crate) fn pre_normalize(input: &str) -> Option<String> {
    let out: String = input
        .chars()
        .map(|c| match c {
            '\u{301C}' | '\u{FF5E}' => 'ー', // 〜 / ～
            _ => c,
        })
        .collect();
    (out != input).then_some(out)
}

/// NFKC 之后：修复分离的浊点/半浊点，并把片假名归一到平假名。
pub(crate) fn post_normalize(input: &str) -> Option<String> {
    let mut owned: Option<String> = None;

    if let Some(fixed) = fix_separated_marks(input) {
        owned = Some(fixed);
    }

    // `wana_kana` 会把 ヷ/ヸ/ヹ/ヺ 直接降成 わ/ゐ/ゑ/を（丢掉浊点），这对
    // “va/vi/ve/vo” 与 “wa/wi/we/wo” 是过度折叠；先拆回 `ワ + U+3099`
    // 等，让 wana 只负责脚本转换，浊点信息保留。
    let current = owned.as_deref().unwrap_or(input);
    if let Some(decomposed) = decompose_va_vo(current) {
        owned = Some(decomposed);
    }

    // 小写假名归一到常规大小（`ァ`→`ア`、`ヶ`→`ケ`）。必须放在脚本转换 *之前*：`wana_kana`
    // 不转换 `ヶ`/`ヵ`，若先跑它、这里再把 `ヶ` 收成 `ケ`，就会留下片假名 `ケ`，与 `1か月`
    // 折叠出的 `か` 仍不相等。先收大小、再交给 wana 统一转平假名，两套写法才落到同一串。
    let current = owned.as_deref().unwrap_or(input);
    if let Some(collapsed) = collapse_small_kana(current) {
        owned = Some(collapsed);
    }

    #[cfg(feature = "jp")]
    {
        let current = owned.as_deref().unwrap_or(input);
        let hira = current.to_hiragana_with_opt(Options {
            // 只做假名折叠，绝不把 ASCII/romaji 转成假名。
            pass_romaji: true,
            // `スーパー` -> `すーぱー`；不做元音展开，避免 `スーパー`/`すうぱあ` 这类
            // 不同拼写被强行合并（索引与查询使用同一规则即可）。
            keep_prolonged_sound_mark: true,
            ..Default::default()
        });
        if hira != current {
            owned = Some(hira);
        }
    }

    // 现代仮名遣い已经废弃的假名：ゐ→い、ゑ→え、ヰ/ヱ 经 wana 转成 ゐ/ゑ 后同样收。
    // 注意：を/ヲ 不收成 お——を 是现代日语仍在用的助词，收了会造成大量假阳性。
    let current = owned.as_deref().unwrap_or(input);
    if let Some(obsolete) = normalize_obsolete_kana(current) {
        owned = Some(obsolete);
    }

    owned
}

/// 小写假名归一到同字的常规大小：`ァ`≡`ア`、`ッ`≡`ツ`、`ャ`≡`ヤ`。
///
/// 半角写法（ｧ、ｯ）不在这里处理：`fold_with` 的 NFKC 步骤在调用本函数前已把它们展开成全角
/// 小字，到这里一律是 `ァ` 一类。关掉宽度折叠时半角也就不折叠，与 `ｱ`/`ア` 保持一致。
///
/// 同一个词常有大/小两种写法（ロリババア / ロリババァ、バッグ / バツグ、ティー / テイー），
/// 而 NFKC 与 `wana_kana` 都只换脚本、不改大小，两者折叠后仍不相等。长音符号 `ー` 不参与；
/// `ヶ`/`ヵ` 这类计数写法（1ヶ月）一并收成 `け`/`か`，代价是把「1か月」和「1ヶ月」也当成
/// 同一串——在这个库里无歧义，换来的是一致性。
fn collapse_small_kana(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    let mut changed = false;
    for c in input.chars() {
        let mapped = match c {
            'ぁ' => 'あ',
            'ぃ' => 'い',
            'ぅ' => 'う',
            'ぇ' => 'え',
            'ぉ' => 'お',
            'っ' => 'つ',
            'ゃ' => 'や',
            'ゅ' => 'ゆ',
            'ょ' => 'よ',
            'ゎ' => 'わ',
            'ゕ' => 'か',
            'ゖ' => 'け',
            'ァ' => 'ア',
            'ィ' => 'イ',
            'ゥ' => 'ウ',
            'ェ' => 'エ',
            'ォ' => 'オ',
            'ッ' => 'ツ',
            'ャ' => 'ヤ',
            'ュ' => 'ユ',
            'ョ' => 'ヨ',
            'ヮ' => 'ワ',
            'ヵ' => 'カ',
            'ヶ' => 'ケ',
            _ => c,
        };
        if mapped != c {
            changed = true;
        }
        out.push(mapped);
    }
    changed.then_some(out)
}

/// 废弃假名 -> 现代假名（按现代仮名遣い）。
fn normalize_obsolete_kana(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    let mut changed = false;
    for c in input.chars() {
        let mapped = match c {
            'ゐ' | 'ヰ' => 'い',
            'ゑ' | 'ヱ' => 'え',
            _ => c,
        };
        if mapped != c {
            changed = true;
        }
        out.push(mapped);
    }
    changed.then_some(out)
}

/// 把片假名专属的 VA/VI/VE/VO 拆成「base + 浊点」，避免 wana_kana 丢浊点。
fn decompose_va_vo(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    let mut changed = false;
    for c in input.chars() {
        match c {
            'ヷ' => {
                out.push_str("ワ\u{3099}");
                changed = true;
            }
            'ヸ' => {
                out.push_str("ヰ\u{3099}");
                changed = true;
            }
            'ヹ' => {
                out.push_str("ヱ\u{3099}");
                changed = true;
            }
            'ヺ' => {
                out.push_str("ヲ\u{3099}");
                changed = true;
            }
            _ => out.push(c),
        }
    }
    changed.then_some(out)
}

#[derive(Clone, Copy)]
enum MarkKind {
    Voiced,
    SemiVoiced,
}

impl MarkKind {
    fn of(c: char) -> Option<Self> {
        match c {
            '\u{3099}' | '\u{309B}' | '\u{FF9E}' => Some(Self::Voiced),
            '\u{309A}' | '\u{309C}' | '\u{FF9F}' => Some(Self::SemiVoiced),
            _ => None,
        }
    }

    const fn combining(self) -> char {
        match self {
            Self::Voiced => '\u{3099}',
            Self::SemiVoiced => '\u{309A}',
        }
    }

    const fn spacing(self) -> char {
        match self {
            Self::Voiced => '\u{309B}',     // ゛
            Self::SemiVoiced => '\u{309C}', // ゜
        }
    }
}

/// 把 NFKC 拆出来的 `kana + [空白] + mark` 重新合成；孤立的 mark 归一到全角间隔
/// 形式 `゛`/`゜`，避免 NFKC 留下的 `空格 + combining` 污染索引。
fn fix_separated_marks(input: &str) -> Option<String> {
    use unicode_normalization::UnicodeNormalization;

    let mut out = String::with_capacity(input.len());
    let mut changed = false;

    for ch in input.chars() {
        let Some(kind) = MarkKind::of(ch) else {
            out.push(ch);
            continue;
        };

        // 去掉 mark 前面由 NFKC 插入的空白（可能不止一个）。
        let ws_start = out.trim_end_matches(|c: char| c.is_whitespace()).len();
        let base = out[..ws_start].chars().next_back();

        match base.filter(|c| is_kana(*c)) {
            Some(base_char) => {
                let pair: String = [base_char, kind.combining()].into_iter().collect();
                let composed: String = pair.as_str().nfc().collect();
                if composed.chars().count() == 1 {
                    out.truncate(ws_start - base_char.len_utf8());
                    out.push_str(&composed);
                } else {
                    // 没有预组合形式的假名（例如 カ+半浊点）：保留区分度，
                    // 但把 mark 归一到间隔形式，保证不同输入形态折叠一致。
                    out.truncate(ws_start);
                    out.push(kind.spacing());
                }
            }
            None => {
                out.truncate(ws_start);
                out.push(kind.spacing());
            }
        }
        changed = true;
    }

    changed.then_some(out)
}

fn is_kana(c: char) -> bool {
    matches!(c,
        '\u{3041}'..='\u{3096}'   // 平假名
        | '\u{30A1}'..='\u{30FA}' // 片假名（含 ヷ-ヺ）
        | '\u{FF66}'..='\u{FF9D}' // 半角片假名
    )
}
