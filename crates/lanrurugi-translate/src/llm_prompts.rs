//! Centralized LLM system prompts.
//!
//! Mirrors `lanrurugi-api`'s own `llm_prompts.rs` (same rationale, same file name, deliberately
//! not shared across the crate boundary — see the decision this file's addition responded to):
//! every non-trivial LLM **system prompt** in this crate should live here so prompt wording,
//! constraints, and output-shape expectations can be reviewed and tuned in one place instead of
//! being scattered across the adapter modules. The request-specific *user* content (glossary
//! matches, known names, tone reference, the batch's own blocks — `TranslationContext::
//! render_prefix`/`TranslationRequest::render_user_message` in `adapter.rs`) stays where it is:
//! it's dynamic per-request data, not prompt wording, and centralizing it here would leak
//! `adapter.rs`'s own types into this module for no benefit.

/// Batched manga-page translation (T028, research.md §14/§15).
///
/// Constant across every request for a given target language, so it sits at the very front of
/// the cacheable prefix (research.md §15/§18).
///
/// 字面出现"json"这几个字是硬性要求：DeepSeek 的 `response_format: json_object` 模式如果 prompt
/// 里完全没提到 json，会直接拒绝请求。
pub(crate) fn translation_system(target_language: &str) -> String {
    format!(
        "你正在把一页漫画上的文字翻译成{target_language}。请逐个按编号翻译每个文本块，忠实\
         传达原意，保留语气和用词的正式/随意程度。只输出一个 json 对象，包含一个\
         \"translations\" 数组，数组每个元素包含三个字段：\"block_id\"（原样照抄输入里的编号，\
         不要改动）、\"translated_text\"（该块的译文）、\"term_kind\"——如果该块的*原文*是一个\
         角色/人物的名字，填 \"person_name\"；如果是其它需要在全书保持一致的专有名称（地名、\
         组织名、作品内的专有概念等），填 \"term\"；其余情况（绝大多数文本块，包括短句、语气词、\
         拟声词）一律填 \"none\"。不要输出任何多余的说明文字，也不要为这个任务展开长篇分步推理\
         ——直接判断并翻译即可。"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_prompt_mentions_json_for_deepseeks_json_object_mode() {
        assert!(translation_system("English")
            .to_lowercase()
            .contains("json"));
    }

    #[test]
    fn the_prompt_asks_for_the_three_term_kind_values() {
        let prompt = translation_system("English");
        assert!(prompt.contains("person_name"));
        assert!(prompt.contains("term"));
        assert!(prompt.contains("none"));
    }
}
