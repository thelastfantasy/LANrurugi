use lanrurugi_equivalence::{Equivalence, FoldConfig};

fn jp() -> Equivalence {
    Equivalence::new(FoldConfig {
        width: true,
        zh: false,
        jp: true,
        latin: false,
    })
}

#[test]
fn halfwidth_kana() {
    let eq = jp();
    assert_eq!(eq.fold("ｶﾞ"), "が");
    assert_eq!(eq.fold("ﾊﾟ"), "ぱ");
    assert_eq!(eq.fold("ｳﾞ"), "ゔ");
    assert_eq!(eq.fold("ｽｰﾊﾟｰ"), "すーぱー");
}

#[test]
fn separated_dakuten_and_handakuten() {
    let eq = jp();
    // 空格分离（OCR / 竖排常见）
    assert_eq!(eq.fold("ｶ ﾞ"), "が");
    assert_eq!(eq.fold("ﾊ ﾟ"), "ぱ");
    // 全角基底 + 半角 mark
    assert_eq!(eq.fold("カﾞ"), "が");
    assert_eq!(eq.fold("ハﾟ"), "ぱ");
    // combining 形式（NFD 常见）
    assert_eq!(eq.fold("か\u{3099}"), "が");
    assert_eq!(eq.fold("は\u{309A}"), "ぱ");
    // 全角间隔 mark（旧 JIS / OCR）
    assert_eq!(eq.fold("か\u{309B}"), "が");
    assert_eq!(eq.fold("は\u{309C}"), "ぱ");
    // 全角间隔 mark + 中间空格
    assert_eq!(eq.fold("か \u{309B}"), "が");
    assert_eq!(eq.fold("は \u{309C}"), "ぱ");
    // 空格 + combining
    assert_eq!(eq.fold("か \u{3099}"), "が");
}

#[test]
fn kana_script_folding() {
    let eq = jp();
    assert_eq!(eq.fold("カタカナ"), "かたかな");
    assert_eq!(eq.fold("スーパー"), "すーぱー");
    // 只折叠假名，不要把 ASCII/romaji 转成假名
    assert_eq!(eq.fold("hello カタカナ"), "hello かたかな");
}

#[test]
fn va_vi_ve_vo_keeps_dakuten_distinction() {
    let eq = jp();
    // wana_kana 默认会把 ヷ 降成 わ；这里应保留为 わ + U+3099，不能与 ワ 混同。
    assert_eq!(eq.fold("ヷ"), eq.fold("ワ\u{3099}"));
    assert_eq!(eq.fold("ヸ"), eq.fold("ヰ\u{3099}"));
    assert_eq!(eq.fold("ヹ"), eq.fold("ヱ\u{3099}"));
    assert_eq!(eq.fold("ヺ"), eq.fold("ヲ\u{3099}"));
    assert_ne!(eq.fold("ヷ"), eq.fold("ワ"));
}

#[test]
fn no_over_normalization() {
    let eq = jp();
    assert_ne!(eq.fold("か"), eq.fold("が"));
    assert_ne!(eq.fold("は"), eq.fold("ぱ"));
    // 孤立 mark 归一到全角间隔形式，但不凭空生成假名
    assert_eq!(eq.fold("Aﾞ"), "A゛");
}

#[test]
fn idempotent() {
    let eq = jp();
    for s in [
        "ｶﾞ",
        "ｶ ﾞ",
        "カﾞ",
        "か\u{3099}",
        "か\u{309B}",
        "か \u{309B}",
        "ﾊ ﾟ",
        "ｽｰﾊﾟｰ",
    ] {
        let once = eq.fold(s);
        let twice = eq.fold(&once);
        assert_eq!(once, twice, "not idempotent for {s:?}");
    }
}

#[test]
fn disabled_jp_leaves_kana_script_alone() {
    let eq = Equivalence::new(FoldConfig {
        width: true,
        zh: false,
        jp: false,
        latin: false,
    });
    // width 基线仍会把半角片假名转全角
    assert_eq!(eq.fold("ｶﾞ"), "ガ");
    // 但不会做片假名->平假名
    assert_eq!(eq.fold("カタカナ"), "カタカナ");
}

#[cfg(feature = "opencc")]
#[test]
fn japanese_kanji_variants() {
    let eq = Equivalence::new(FoldConfig {
        width: true,
        zh: false,
        jp: true,
        latin: false,
    });
    // 日文新字体 / 旧字体（旧 JIS / 战前写法）
    assert_eq!(eq.fold("発"), eq.fold("發"));
    assert_eq!(eq.fold("竜"), eq.fold("龍"));
    assert_eq!(eq.fold("沢"), eq.fold("澤"));
}
