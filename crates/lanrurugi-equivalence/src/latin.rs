//! 拉丁变音符号折叠。
//!
//! 关键点（issue #108 调研）：U+3099 / U+309A 日文浊点的
//! `canonical_combining_class` 与拉丁 combining mark 一样都是 8，绝不能无差别
//! 剔除；这里只在分解出的 base 落在拉丁区段时才丢弃 combining mark。

use unicode_normalization::char::{canonical_combining_class, decompose_canonical};

pub(crate) fn fold(input: &str) -> Option<String> {
    let mut out = String::with_capacity(input.len());
    let mut changed = false;

    for ch in input.chars() {
        let mut base = String::new();
        let mut latin_base = false;
        let mut has_mark = false;

        decompose_canonical(ch, |d| {
            if canonical_combining_class(d) == 0 {
                if is_latin(d) {
                    latin_base = true;
                }
                base.push(d);
            } else {
                has_mark = true;
            }
        });

        if has_mark && latin_base {
            out.push_str(&base);
            changed = true;
        } else {
            out.push(ch);
        }
    }

    changed.then_some(out)
}

fn is_latin(c: char) -> bool {
    matches!(c,
        'A'..='Z'
        | 'a'..='z'
        | '\u{00C0}'..='\u{024F}'   // Latin-1 Supplement letters + Latin Extended-A/B
        | '\u{1E00}'..='\u{1EFF}'   // Latin Extended Additional
        | '\u{2C60}'..='\u{2C7F}'   // Latin Extended-C
        | '\u{A720}'..='\u{A7FF}'   // Latin Extended-D
        | '\u{AB30}'..='\u{AB6F}'   // Latin Extended-E
    )
}
