use lanrurugi_equivalence::{Equivalence, FoldConfig};

fn latin() -> Equivalence {
    Equivalence::new(FoldConfig {
        width: true,
        zh: false,
        jp: false,
        latin: true,
    })
}

#[test]
fn diacritics_fold() {
    let eq = latin();
    assert_eq!(eq.fold("MÄR"), "MAR");
    assert_eq!(eq.fold("Café"), "Cafe");
    assert_eq!(eq.fold("naïve"), "naive");
}

#[test]
fn does_not_eat_japanese_dakuten() {
    // 回归测试：U+3099/U+309A 的 combining class 也是 8，不能当拉丁 mark 删掉。
    let eq = latin();
    assert_eq!(eq.fold("が"), "が");
    assert_eq!(eq.fold("ガ"), "ガ");
    assert_eq!(eq.fold("パ"), "パ");
    assert_eq!(eq.fold("ヴ"), "ヴ");
}

#[test]
fn jp_and_latin_can_coexist() {
    let eq = Equivalence::new(FoldConfig {
        width: true,
        zh: false,
        jp: true,
        latin: true,
    });
    assert_eq!(eq.fold("Caféｶﾞ"), "Cafeが");
}
