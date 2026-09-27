#![cfg(feature = "zh")]

use lanrurugi_equivalence::{Equivalence, FoldConfig};

fn zh() -> Equivalence {
    Equivalence::new(FoldConfig {
        width: true,
        zh: true,
        jp: false,
        latin: false,
    })
}

#[test]
fn traditional_to_simplified() {
    let eq = zh();
    assert_eq!(eq.fold("龍"), "龙");
    assert_eq!(eq.fold("臺灣"), "台湾");
    assert_eq!(eq.fold("發"), "发");
    // 简体侧保持不变，所以双向都能命中
    assert_eq!(eq.fold("龙"), "龙");
    assert_eq!(eq.fold("台湾"), "台湾");
}

#[test]
fn zh_does_not_touch_kana() {
    let eq = zh();
    assert_eq!(eq.fold("カタカナ"), "カタカナ");
}

#[test]
fn zh_and_jp_can_coexist() {
    let eq = Equivalence::new(FoldConfig::all());
    assert_eq!(eq.fold("ｶ ﾞ龍"), "が龙");
}

#[test]
fn mixed_simplified_traditional_is_canonicalized() {
    let eq = zh();
    // 同一句话，繁简混用程度不同，canonical 后必须一致
    assert_eq!(eq.fold("龍与虎"), eq.fold("龙與虎"));
    assert_eq!(eq.fold("龍与虎"), "龙与虎");
    assert_eq!(eq.fold("臺灣与龍"), eq.fold("台湾与龙"));
    assert_eq!(eq.fold("發展与髮型"), eq.fold("发展与发型"));
    // 纯繁体 / 纯简体
    assert_eq!(eq.fold("龍與虎"), eq.fold("龙与虎"));
}

#[test]
fn mixed_with_japanese_kana_title() {
    let eq = Equivalence::new(FoldConfig::all());
    assert_eq!(eq.fold("龍ガ如く"), eq.fold("龙が如く"));
    assert_eq!(eq.fold("龍ガ如く"), "龙が如く");
}

#[cfg(feature = "opencc")]
#[test]
fn japanese_kanji_with_zh_goes_to_simplified() {
    let eq = Equivalence::new(FoldConfig::all());
    assert_eq!(eq.fold("発"), "发");
    assert_eq!(eq.fold("發"), "发");
    assert_eq!(eq.fold("竜"), "龙");
    assert_eq!(eq.fold("龍"), "龙");
    assert_eq!(eq.fold("龙"), "龙");
    // 龍 / 竜 / 龙 三个形态必须互相命中
    assert_eq!(eq.fold("竜"), eq.fold("龍"));
    assert_eq!(eq.fold("竜"), eq.fold("龙"));
    assert_eq!(eq.fold("龍"), eq.fold("龙"));
    assert_eq!(eq.fold("沢"), "泽");
    assert_eq!(eq.fold("澤"), "泽");
    // 日文“新字体 / 旧字体”与中文“简体 / 繁体”三方互通
    assert_eq!(eq.fold("辺"), "边");
    assert_eq!(eq.fold("邊"), "边");
    assert_eq!(eq.fold("図"), "图");
    assert_eq!(eq.fold("圖"), "图");
    assert_eq!(eq.fold("売"), "卖");
    assert_eq!(eq.fold("賣"), "卖");
    // 中日混用里中日两边都用简体形态的情况
    assert_eq!(eq.fold("辺"), eq.fold("边"));
    assert_eq!(eq.fold("図"), eq.fold("图"));
    assert_eq!(eq.fold("売"), eq.fold("卖"));
}

#[cfg(feature = "opencc")]
#[test]
fn mixed_cn_jp_needs_both_languages_on() {
    // 只开 zh：日文新字体 辺 不会变成中文简化 边 -> 跨语言会漏
    let zh_only = Equivalence::new(FoldConfig {
        width: true,
        zh: true,
        jp: false,
        latin: false,
    });
    assert_ne!(zh_only.fold("辺"), zh_only.fold("边"));

    // 只开 jp：中文简化 边 不会变成日文旧字体 邊 -> 跨语言也会漏
    let jp_only = Equivalence::new(FoldConfig {
        width: true,
        zh: false,
        jp: true,
        latin: false,
    });
    assert_ne!(jp_only.fold("辺"), jp_only.fold("边"));

    // 同时开 zh + jp：两边都收敛到中文简体 边
    let both = Equivalence::new(FoldConfig::all());
    assert_eq!(both.fold("辺"), both.fold("边"));
    assert_eq!(both.fold("辺"), "边");

    // 竜 / 龍：只开 zh 时 龍->龙，但 竜 是日文新字体，zh 不认识 -> 不互通
    assert_ne!(zh_only.fold("竜"), zh_only.fold("龍"));
    // 开 jp 后 Jp2t 把 竜 收敛到 龍 -> 互通
    assert_eq!(jp_only.fold("竜"), jp_only.fold("龍"));
    // zh + jp 一起时继续收敛到中文简体 龙
    assert_eq!(both.fold("竜"), "龙");
    assert_eq!(both.fold("龍"), "龙");
}

#[test]
fn dakuten_example_皇女達_matches_simplified_皇女达() {
    let eq = zh();
    assert_eq!(eq.fold("皇女達"), "皇女达");
    assert_eq!(eq.fold("皇女达"), "皇女达");
    assert_eq!(eq.fold("皇女達"), eq.fold("皇女达"));
}
