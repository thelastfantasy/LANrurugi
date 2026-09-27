//! 中日汉字互通：简繁、日文新/旧字体、以及三者的交叉组合。
//!
//! 这部分依赖 OpenCC（`opencc` feature，`zh` 会带上它）；只有 `jp` feature 时跳过。
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
fn japanese_shinjitai_and_kyujitai() {
    let eq = jp_only();
    assert_eq!(eq.fold("発"), eq.fold("發"));
    assert_eq!(eq.fold("竜"), eq.fold("龍"));
    assert_eq!(eq.fold("沢"), eq.fold("澤"));
}

#[test]
fn cn_jp_three_way_variants() {
    let eq = all();
    // 日文新字体 / 中文繁体 / 中文简体 三方互通
    for (a, b, c) in [
        ("発", "發", "发"),
        ("竜", "龍", "龙"),
        ("沢", "澤", "泽"),
        ("辺", "邊", "边"),
        ("図", "圖", "图"),
        ("売", "賣", "卖"),
    ] {
        assert_eq!(eq.fold(a), c, "{a} should canonicalize to {c}");
        assert_eq!(eq.fold(b), c, "{b} should canonicalize to {c}");
        assert_eq!(eq.fold(a), eq.fold(b));
        assert_eq!(eq.fold(b), eq.fold(c));
    }
}

#[test]
fn mixed_cn_jp_title_is_canonicalized() {
    let eq = all();
    assert_eq!(eq.fold("龍ガ如く"), "龙が如く");
    assert_eq!(eq.fold("竜ガ如く"), "龙が如く");
    assert_eq!(eq.fold("龙が如く"), "龙が如く");
    assert_eq!(eq.fold("龍ガ如く"), eq.fold("龙が如く"));
    assert_eq!(eq.fold("ｶ ﾞ龍"), "が龙");
}

#[test]
fn cjk_compat_ideographs_with_canonical_decomposition() {
    let eq = all();
    // 这些兼容汉字有 canonical decomposition，NFKC 会统一到对应 unified ideograph。
    for (compat, unified) in [
        ("\u{FA19}", "神"),
        ("\u{FA10}", "塚"),
        ("\u{FA1A}", "祥"),
        ("\u{FA12}", "晴"),
    ] {
        assert_eq!(
            eq.fold(compat),
            eq.fold(unified),
            "U+{:04X} != {unified}",
            compat.chars().next().unwrap() as u32
        );
    }
}

#[test]
fn common_hanzi_same_codepoint_stays_same() {
    let eq = all();
    // 才 在中日输入法里通常都是 U+624D；同一个码位本来就不需要归一化。
    assert_eq!(eq.fold("才"), "才");
    assert_eq!(eq.fold("\u{624D}"), "才");
}
