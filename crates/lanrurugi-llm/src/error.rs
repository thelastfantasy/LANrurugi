//! One failure vocabulary for every model call (`specs/014-ai-harness-hardening/contracts/
//! llm-call-contract.md`). Each error keeps the exact message its call site rendered before this
//! type existed, so adopting it changes no user-visible text; the class is what callers, the retry
//! policy and the usage ledger branch on.

use std::fmt;
use std::time::Duration;

/// The class of a model-call failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LlmErrorClass {
    NotConfigured,
    Auth,
    Balance,
    RateLimited,
    /// Network failure, 5xx, connection reset, unreadable body.
    Transient,
    Timeout,
    /// Unparseable or incomplete response, or a request the provider refused as malformed (other
    /// 4xx). Only the truncation case is retried.
    Malformed,
    /// A deterministic validator refused the output.
    Rejected,
    /// The caller's cancellation signal fired. Not a failure.
    Cancelled,
}

impl LlmErrorClass {
    /// The stable name used by the usage ledger and HTTP error bodies.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Auth => "auth",
            Self::Balance => "balance",
            Self::RateLimited => "rate_limited",
            Self::Transient => "transient",
            Self::Timeout => "timeout",
            Self::Malformed => "malformed",
            Self::Rejected => "rejected",
            Self::Cancelled => "cancelled",
        }
    }
}

/// A failed model call: its class plus the message shown to the user.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LlmError {
    class: LlmErrorClass,
    message: String,
    retry_after: Option<Duration>,
    /// Set only for a `Malformed` stream that was cut off, the one malformed case worth retrying.
    truncated: bool,
}

impl LlmError {
    fn new(class: LlmErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
            retry_after: None,
            truncated: false,
        }
    }

    pub fn class(&self) -> LlmErrorClass {
        self.class
    }

    pub fn message(&self) -> &str {
        &self.message
    }

    pub fn retry_after(&self) -> Option<Duration> {
        self.retry_after
    }

    pub fn is_truncation(&self) -> bool {
        self.truncated
    }

    /// Whether the retry policy may try again: transient, rate-limited, and truncated streams.
    pub fn is_retryable(&self) -> bool {
        match self.class {
            LlmErrorClass::Transient | LlmErrorClass::RateLimited => true,
            LlmErrorClass::Malformed => self.truncated,
            _ => false,
        }
    }

    pub fn not_configured() -> Self {
        Self::new(
            LlmErrorClass::NotConfigured,
            "DeepSeek API key not configured",
        )
    }

    /// A non-2xx chat-completions response.
    pub fn from_status(status: u16, provider_message: &str, retry_after: Option<Duration>) -> Self {
        let msg = provider_message;
        match status {
            401 => Self::new(
                LlmErrorClass::Auth,
                format!("LLM: API Key 无效 (401) — {msg}"),
            ),
            402 => Self::new(
                LlmErrorClass::Balance,
                format!("LLM: 账户余额不足 (402) — {msg}"),
            ),
            429 => Self {
                retry_after,
                ..Self::new(
                    LlmErrorClass::RateLimited,
                    format!("LLM: 请求频率超限 (429) — {msg}"),
                )
            },
            500 | 503 => Self::new(
                LlmErrorClass::Transient,
                format!("LLM: 服务暂时不可用 ({status}) — {msg}"),
            ),
            s if s >= 500 => Self::new(
                LlmErrorClass::Transient,
                format!("LLM: API 错误 ({status}) — {msg}"),
            ),
            _ => Self::new(
                LlmErrorClass::Malformed,
                format!("LLM: API 错误 ({status}) — {msg}"),
            ),
        }
    }

    /// A non-2xx response from the `/user/balance` pre-flight.
    pub fn from_balance_status(status: u16, provider_message: &str) -> Self {
        let msg = provider_message;
        match status {
            401 => Self::new(
                LlmErrorClass::Auth,
                format!("LLM: API Key 无效 (401) — {msg}"),
            ),
            402 => Self::new(
                LlmErrorClass::Balance,
                format!("LLM: 账户余额不足 (402) — {msg}"),
            ),
            s => Self::new(
                if s >= 500 {
                    LlmErrorClass::Transient
                } else {
                    LlmErrorClass::Auth
                },
                format!("LLM: 余额检查失败 ({s}) — {msg}"),
            ),
        }
    }

    pub fn balance_unavailable() -> Self {
        Self::new(
            LlmErrorClass::Balance,
            "LLM: DeepSeek 账户余额不足或账户不可用",
        )
    }

    pub fn balance_request_failed(e: impl fmt::Display) -> Self {
        Self::new(
            LlmErrorClass::Transient,
            format!("LLM: 余额检查请求失败: {e}"),
        )
    }

    pub fn request_failed(e: impl fmt::Display) -> Self {
        Self::new(LlmErrorClass::Transient, format!("AI API 请求失败: {e}"))
    }

    pub fn body_read_failed(e: impl fmt::Display) -> Self {
        Self::new(
            LlmErrorClass::Transient,
            format!("AI API 响应读取失败: {e}"),
        )
    }

    pub fn stream_read_failed(e: impl fmt::Display) -> Self {
        Self::new(
            LlmErrorClass::Transient,
            format!("AI API 流式响应读取失败: {e}"),
        )
    }

    pub fn not_json(e: impl fmt::Display) -> Self {
        Self::new(
            LlmErrorClass::Malformed,
            format!("AI 返回了非 JSON 数据: {e}"),
        )
    }

    pub fn missing_content() -> Self {
        Self::new(
            LlmErrorClass::Malformed,
            "AI 返回格式异常，缺少 content 字段",
        )
    }

    pub fn bad_tool_calls(e: impl fmt::Display) -> Self {
        Self::new(
            LlmErrorClass::Malformed,
            format!("AI 返回的 tool_calls 格式异常: {e}"),
        )
    }

    pub fn neither_tool_calls_nor_content() -> Self {
        Self::new(
            LlmErrorClass::Malformed,
            "AI 返回格式异常，既无 tool_calls 也无 content 字段",
        )
    }

    /// A stream cut off mid-response or truncated by reasoning; retried once.
    pub fn truncated() -> Self {
        Self {
            truncated: true,
            ..Self::new(
                LlmErrorClass::Malformed,
                "AI 返回的内容不完整（连接中断或响应被截断），已重试仍未成功，请再次尝试",
            )
        }
    }

    pub fn unparseable(text: &str) -> Self {
        Self::new(
            LlmErrorClass::Malformed,
            format!("AI 返回了无法解析的格式: {text}"),
        )
    }

    pub fn timeout() -> Self {
        Self::new(LlmErrorClass::Timeout, "AI 请求超时")
    }

    pub fn cancelled() -> Self {
        Self::new(LlmErrorClass::Cancelled, "已取消")
    }

    /// A deterministic validator's refusal, carrying the validator's own message.
    pub fn rejected(reason: impl Into<String>) -> Self {
        Self::new(LlmErrorClass::Rejected, reason)
    }
}

impl fmt::Display for LlmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for LlmError {}

/// Lossless for every existing call site that returns `Result<_, String>`.
impl From<LlmError> for String {
    fn from(e: LlmError) -> Self {
        e.message
    }
}

/// `Retry-After` in seconds; HTTP-date values are ignored (DeepSeek sends seconds).
pub fn parse_retry_after(value: Option<&str>) -> Option<Duration> {
    value?.trim().parse::<u64>().ok().map(Duration::from_secs)
}
