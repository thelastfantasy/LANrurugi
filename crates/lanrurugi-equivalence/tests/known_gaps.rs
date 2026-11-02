//! 已知仍未覆盖的边界，全部 `#[ignore]`。
//!
//! 目的：把「以为支持了、其实没支持」的情况显式记录下来。以后真正实现时去掉
//! `#[ignore]` 即可当回归测试。
#![cfg(feature = "opencc")]

use lanrurugi_equivalence::{Equivalence, FoldConfig};

fn all() -> Equivalence {
    Equivalence::new(FoldConfig::all())
}

fn jp_only() -> Equivalence {
    Equivalence::new(FoldConfig {
        width: true,
        zh: false,
        jp: true,
        latin: false,
    })
}

#[test]
#[ignore = "known gap: 歴史的仮名遣い 词形（けふ→きょう、やう→よう 等）尚未实现"]
fn historical_kana_usage_forms() {
    let eq = all();
    assert_eq!(eq.fold("けふ"), eq.fold("きょう"));
    assert_eq!(eq.fold("てふ"), eq.fold("ちょう"));
    assert_eq!(eq.fold("やう"), eq.fold("よう"));
    assert_eq!(eq.fold("くわ"), eq.fold("か"));
    assert_eq!(eq.fold("ぐわ"), eq.fold("が"));
    assert_eq!(eq.fold("あはれ"), eq.fold("あわれ"));
    assert_eq!(eq.fold("こひ"), eq.fold("こい"));
}

#[test]
#[ignore = "known gap: 无 canonical decomposition 的日本兼容汉字未覆盖（U+FA11/U+FA20/U+FA21 等）"]
fn jp_compat_ideographs_without_decomposition() {
    let eq = all();
    assert_eq!(eq.fold("\u{FA11}"), eq.fold("崎"));
    assert_eq!(eq.fold("\u{FA20}"), eq.fold("芦"));
    assert_eq!(eq.fold("\u{FA21}"), eq.fold("響"));
}

#[test]
#[ignore = "known gap: 地域词汇差异（台灣「計程車」vs 大陆「出租车」）需要 Tw2s/Hk2s/Tw2sp"]
fn regional_chinese_vocabulary() {
    let eq = all();
    assert_eq!(eq.fold("計程車"), eq.fold("出租车"));
    assert_eq!(eq.fold("腳踏車"), eq.fold("自行车"));
}

#[test]
#[ignore = "known behavior: OpenCC Jp2t 会把 jp-only 的 才 归到 纔；zh 同时打开时才回到 才"]
fn jp2t_over_normalization_cai() {
    let eq = jp_only();
    assert_eq!(eq.fold("才"), "才");
}
