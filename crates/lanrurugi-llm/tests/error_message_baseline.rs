//! SC-008 guard: every user-visible AI error string, captured from the two status-code ladders in
//! `lib.rs` before `014-ai-harness-hardening` replaced them, must render byte-for-byte the same
//! through the new [`LlmError`] vocabulary.
//!
//! | Source (before) | Rendered string |
//! | --- | --- |
//! | missing key | `DeepSeek API key not configured` |
//! | 401 | `LLM: API Key 无效 (401) — {msg}` |
//! | 402 | `LLM: 账户余额不足 (402) — {msg}` |
//! | 429 | `LLM: 请求频率超限 (429) — {msg}` |
//! | 500 / 503 | `LLM: 服务暂时不可用 ({code}) — {msg}` |
//! | other status | `LLM: API 错误 ({code}) — {msg}` |
//! | request failed | `AI API 请求失败: {e}` |
//! | body read failed | `AI API 响应读取失败: {e}` |
//! | stream read failed | `AI API 流式响应读取失败: {e}` |
//! | non-JSON body | `AI 返回了非 JSON 数据: {e}` |
//! | no content | `AI 返回格式异常，缺少 content 字段` |
//! | bad tool_calls | `AI 返回的 tool_calls 格式异常: {e}` |
//! | neither | `AI 返回格式异常，既无 tool_calls 也无 content 字段` |
//! | truncated twice | `AI 返回的内容不完整（连接中断或响应被截断），已重试仍未成功，请再次尝试` |
//! | `json_chat` unparseable | `AI 返回了无法解析的格式: {text}` |
//! | balance request failed | `LLM: 余额检查请求失败: {e}` |
//! | balance other status | `LLM: 余额检查失败 ({code}) — {msg}` |
//! | balance unavailable | `LLM: DeepSeek 账户余额不足或账户不可用` |

use lanrurugi_llm::{LlmError, LlmErrorClass};

fn shown(e: LlmError) -> String {
    e.to_string()
}

#[test]
fn status_ladder_strings_are_unchanged() {
    let cases = [
        (
            401,
            "LLM: API Key 无效 (401) — bad key",
            LlmErrorClass::Auth,
        ),
        (
            402,
            "LLM: 账户余额不足 (402) — bad key",
            LlmErrorClass::Balance,
        ),
        (
            429,
            "LLM: 请求频率超限 (429) — bad key",
            LlmErrorClass::RateLimited,
        ),
        (
            500,
            "LLM: 服务暂时不可用 (500) — bad key",
            LlmErrorClass::Transient,
        ),
        (
            503,
            "LLM: 服务暂时不可用 (503) — bad key",
            LlmErrorClass::Transient,
        ),
        (
            502,
            "LLM: API 错误 (502) — bad key",
            LlmErrorClass::Transient,
        ),
        (
            400,
            "LLM: API 错误 (400) — bad key",
            LlmErrorClass::Malformed,
        ),
    ];
    for (status, expected, class) in cases {
        let err = LlmError::from_status(status, "bad key", None);
        assert_eq!(err.class(), class, "status {status}");
        assert_eq!(shown(err), expected, "status {status}");
    }
}

#[test]
fn transport_and_shape_strings_are_unchanged() {
    assert_eq!(
        shown(LlmError::not_configured()),
        "DeepSeek API key not configured"
    );
    assert_eq!(
        shown(LlmError::request_failed("boom")),
        "AI API 请求失败: boom"
    );
    assert_eq!(
        shown(LlmError::body_read_failed("boom")),
        "AI API 响应读取失败: boom"
    );
    assert_eq!(
        shown(LlmError::stream_read_failed("boom")),
        "AI API 流式响应读取失败: boom"
    );
    assert_eq!(
        shown(LlmError::not_json("boom")),
        "AI 返回了非 JSON 数据: boom"
    );
    assert_eq!(
        shown(LlmError::missing_content()),
        "AI 返回格式异常，缺少 content 字段"
    );
    assert_eq!(
        shown(LlmError::bad_tool_calls("boom")),
        "AI 返回的 tool_calls 格式异常: boom"
    );
    assert_eq!(
        shown(LlmError::neither_tool_calls_nor_content()),
        "AI 返回格式异常，既无 tool_calls 也无 content 字段"
    );
    assert_eq!(
        shown(LlmError::truncated()),
        "AI 返回的内容不完整（连接中断或响应被截断），已重试仍未成功，请再次尝试"
    );
    assert_eq!(
        shown(LlmError::unparseable("{oops")),
        "AI 返回了无法解析的格式: {oops"
    );
}

#[test]
fn balance_strings_are_unchanged() {
    assert_eq!(
        shown(LlmError::balance_request_failed("boom")),
        "LLM: 余额检查请求失败: boom"
    );
    assert_eq!(
        shown(LlmError::from_balance_status(403, "nope")),
        "LLM: 余额检查失败 (403) — nope"
    );
    assert_eq!(
        shown(LlmError::from_balance_status(401, "nope")),
        "LLM: API Key 无效 (401) — nope"
    );
    assert_eq!(
        shown(LlmError::balance_unavailable()),
        "LLM: DeepSeek 账户余额不足或账户不可用"
    );
}

#[test]
fn converting_to_string_is_lossless() {
    let err = LlmError::from_status(429, "slow down", None);
    let expected = err.to_string();
    let as_string: String = err.into();
    assert_eq!(as_string, expected);
}
