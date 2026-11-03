//! 假名等价形态：平/片、相邻/分离浊点、半角/全角，都要收敛到同一个 canonical key。
#![cfg(feature = "jp")]

use lanrurugi_equivalence::{Equivalence, FoldConfig};

fn jp() -> Equivalence {
    Equivalence::new(FoldConfig {
        width: true,
        zh: false,
        jp: true,
        latin: false,
    })
}

/// 断言一组写法都等价，并且都等于 expected。
fn all_equal(eq: &Equivalence, expected: &str, variants: &[(&str, &str)]) {
    for (variant, label) in variants {
        let got = eq.fold(variant);
        assert_eq!(
            got, expected,
            "{label}: {variant:?} -> {got:?}, expected {expected:?}"
        );
    }
}

#[test]
fn ga_variants() {
    let eq = jp();
    // 平假名 が
    // 片假名 ガ
    // か + combining dakuten (NFD)
    // か + 全角间隔浊点
    // か + 空格 + combining
    // か + 空格 + 全角间隔浊点
    // 半角 ｶﾞ / ｶ + 空格 + 半角浊点 / 全角カ + 半角浊点
    all_equal(
        &eq,
        "が",
        &[
            ("が", "precomposed hiragana"),
            ("ガ", "precomposed katakana"),
            ("か\u{3099}", "ka + combining dakuten"),
            ("か\u{309B}", "ka + fullwidth spacing dakuten"),
            ("か \u{3099}", "ka + space + combining dakuten"),
            ("か \u{309B}", "ka + space + fullwidth spacing dakuten"),
            ("ｶﾞ", "halfwidth ka + halfwidth dakuten"),
            ("ｶ \u{FF9E}", "halfwidth ka + space + halfwidth dakuten"),
            ("カ\u{FF9E}", "fullwidth ka + halfwidth dakuten"),
        ],
    );
}

#[test]
fn pa_variants() {
    let eq = jp();
    all_equal(
        &eq,
        "ぱ",
        &[
            ("ぱ", "precomposed hiragana"),
            ("パ", "precomposed katakana"),
            ("は\u{309A}", "ha + combining handakuten"),
            ("は\u{309C}", "ha + fullwidth spacing handakuten"),
            ("は \u{309A}", "ha + space + combining handakuten"),
            ("は \u{309C}", "ha + space + fullwidth spacing handakuten"),
            ("ﾊﾟ", "halfwidth ha + halfwidth handakuten"),
            ("ﾊ \u{FF9F}", "halfwidth ha + space + halfwidth handakuten"),
            ("ハ\u{FF9F}", "fullwidth ha + halfwidth handakuten"),
        ],
    );
}

#[test]
fn vu_variants() {
    let eq = jp();
    all_equal(
        &eq,
        "ゔ",
        &[
            ("ゔ", "hiragana vu"),
            ("ヴ", "katakana vu"),
            ("う\u{3099}", "u + combining dakuten"),
            ("ｳﾞ", "halfwidth u + halfwidth dakuten"),
            ("ウ\u{FF9E}", "fullwidth u + halfwidth dakuten"),
        ],
    );
}

#[test]
fn other_rows_also_fold() {
    let eq = jp();
    for (variants, expected) in [
        (&["ざ", "ザ", "ｻﾞ"][..], "ざ"),
        (&["だ", "ダ", "ﾀﾞ"][..], "だ"),
        (&["ば", "バ", "ﾊﾞ"][..], "ば"),
        (&["ぴ", "ピ", "ﾋﾟ"][..], "ぴ"),
    ] {
        for v in variants {
            assert_eq!(eq.fold(v), expected, "{v:?}");
        }
    }
}

#[test]
fn kana_script_variants() {
    let eq = jp();
    for v in ["カタカナ", "かたかな", "ｶﾀｶﾅ"] {
        assert_eq!(eq.fold(v), "かたかな", "{v:?}");
    }
    for v in ["スーパー", "すーぱー", "ｽｰﾊﾟｰ"] {
        assert_eq!(eq.fold(v), "すーぱー", "{v:?}");
    }
}
