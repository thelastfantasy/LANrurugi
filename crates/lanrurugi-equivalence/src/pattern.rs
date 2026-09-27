use std::borrow::Cow;

use crate::{Equivalence, FoldConfig};

/// 折叠搜索 pattern，保留 `*` / `?` 通配符语义：只折叠字面量片段。
pub(crate) fn fold_pattern_with<'a>(
    eq: &Equivalence,
    pattern: &'a str,
    config: FoldConfig,
) -> Cow<'a, str> {
    if !pattern.contains('*') && !pattern.contains('?') {
        return eq.fold_with(pattern, config);
    }

    let mut out = String::with_capacity(pattern.len());
    let mut start = 0usize;

    for (i, ch) in pattern.char_indices() {
        if ch == '*' || ch == '?' {
            if start < i {
                out.push_str(&eq.fold_with(&pattern[start..i], config));
            }
            out.push(ch);
            start = i + ch.len_utf8();
        }
    }
    if start < pattern.len() {
        out.push_str(&eq.fold_with(&pattern[start..], config));
    }

    Cow::Owned(out)
}
