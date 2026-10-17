use lanrurugi_equivalence::{Equivalence, FoldConfig};

#[test]
fn pattern_keeps_wildcards() {
    let eq = Equivalence::new(FoldConfig::new(false, true));
    assert_eq!(eq.fold_pattern("ｶﾞ*"), "が*");
    assert_eq!(eq.fold_pattern("?ｶ ﾞ"), "?が");
    assert_eq!(eq.fold_pattern("*ﾊ ﾟ*"), "*ぱ*");
}

#[test]
fn exact_pattern_has_no_wildcard() {
    let eq = Equivalence::new(FoldConfig::new(false, true));
    assert_eq!(eq.fold_pattern("カタカナ"), "かたかな");
}
