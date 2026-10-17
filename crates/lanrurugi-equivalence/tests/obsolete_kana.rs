//! 現代仮名遣いで廃止された旧仮名の正規化（漫画の古風な台詞など）。
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

#[test]
fn obsolete_hiragana_and_katakana_fold_to_modern() {
    let eq = jp();
    assert_eq!(eq.fold("ゐ"), "い");
    assert_eq!(eq.fold("ゑ"), "え");
    assert_eq!(eq.fold("ヰ"), "い");
    assert_eq!(eq.fold("ヱ"), "え");
    assert_eq!(eq.fold("ゐる"), "いる");
    assert_eq!(eq.fold("ゑびす"), "えびす");
}

#[test]
fn wo_is_not_folded_to_o() {
    let eq = jp();
    // を は現代日本語でも助詞として使うので、お に寄せると假陽性が大量に出る。
    assert_eq!(eq.fold("を"), "を");
    assert_eq!(eq.fold("ヲ"), "を");
    assert_ne!(eq.fold("を"), eq.fold("お"));
}

#[test]
fn ligature_kana_expand_via_nfkc() {
    let eq = jp();
    assert_eq!(eq.fold("ゟ"), "より");
    assert_eq!(eq.fold("ヿ"), "こと");
}

#[test]
fn obsolete_kana_is_idempotent() {
    let eq = jp();
    for s in [
        "ゐ",
        "ゑ",
        "ヰ",
        "ヱ",
        "ゐる",
        "ゑびす",
        "を",
        "ヲ",
        "ゟ",
        "ヿ",
    ] {
        let once = eq.fold(s);
        let twice = eq.fold(&once);
        assert_eq!(once, twice, "not idempotent for {s:?}");
    }
}
