//! Prefix stability for multi-round conversations. The provider's prompt cache is a prefix cache:
//! a round is cheap when its request is the previous round's request plus new messages at the end.
//! A [`Conversation`] on the [`crate::CallSpec`] makes the seam check that before every round,
//! the way a loop's history is built — append only, never rewritten, same tools — so a later
//! change that breaks it shows up in the log and the usage ledger instead of only on the bill.
//! A request that deliberately changes the prompt (a final JSON-only round without tools) simply
//! does not carry the conversation.

use std::sync::{Arc, Mutex};

use serde_json::Value;

/// Why a round does not extend the previous one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrefixBreak {
    /// The tool declarations differ (they are part of the cached prompt).
    ToolsChanged,
    /// The message at this index differs from the previous round's.
    MessageRewritten(usize),
    /// The history is shorter than the previous round's.
    HistoryShrunk,
}

impl PrefixBreak {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ToolsChanged => "tools_changed",
            Self::MessageRewritten(_) => "message_rewritten",
            Self::HistoryShrunk => "history_shrunk",
        }
    }
}

/// Whether `next` extends `prev`: identical tools, and `prev`'s messages as `next`'s first ones.
pub fn prefix_break(
    prev_tools: &Value,
    prev: &[Value],
    tools: &Value,
    next: &[Value],
) -> Option<PrefixBreak> {
    if prev_tools != tools {
        return Some(PrefixBreak::ToolsChanged);
    }
    if next.len() < prev.len() {
        return Some(PrefixBreak::HistoryShrunk);
    }
    prev.iter()
        .zip(next)
        .position(|(a, b)| a != b)
        .map(PrefixBreak::MessageRewritten)
}

/// The previous round's tools and messages.
type LastRequest = Option<(Value, Vec<Value>)>;

/// One multi-round conversation's last request, shared by its rounds.
#[derive(Debug, Clone, Default)]
pub struct Conversation {
    last: Arc<Mutex<LastRequest>>,
    breaks: Arc<Mutex<u32>>,
}

impl Conversation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records this round's request and reports whether it failed to extend the previous one.
    /// The first round always passes.
    pub fn advance(&self, tools: &Value, messages: &[Value]) -> Option<PrefixBreak> {
        let mut last = self.last.lock().unwrap_or_else(|e| e.into_inner());
        let found = last
            .as_ref()
            .and_then(|(prev_tools, prev)| prefix_break(prev_tools, prev, tools, messages));
        *last = Some((tools.clone(), messages.to_vec()));
        if found.is_some() {
            *self.breaks.lock().unwrap_or_else(|e| e.into_inner()) += 1;
        }
        found
    }

    /// Breaks seen so far in this conversation.
    pub fn breaks(&self) -> u32 {
        *self.breaks.lock().unwrap_or_else(|e| e.into_inner())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msgs(v: &[&str]) -> Vec<Value> {
        v.iter()
            .map(|c| json!({ "role": "user", "content": c }))
            .collect()
    }

    #[test]
    fn appending_messages_extends_the_prefix() {
        let tools = json!([{ "name": "fetch_page" }]);
        let c = Conversation::new();
        assert_eq!(c.advance(&tools, &msgs(&["sys", "ask"])), None);
        assert_eq!(
            c.advance(&tools, &msgs(&["sys", "ask", "call", "result"])),
            None
        );
        assert_eq!(c.breaks(), 0);
    }

    #[test]
    fn rewriting_shrinking_or_retooling_is_a_break() {
        let tools = json!([{ "name": "fetch_page" }]);
        let c = Conversation::new();
        c.advance(&tools, &msgs(&["sys", "ask", "call"]));
        assert_eq!(
            c.advance(&tools, &msgs(&["sys", "ask (edited)", "call", "more"])),
            Some(PrefixBreak::MessageRewritten(1))
        );
        assert_eq!(
            c.advance(&tools, &msgs(&["sys"])),
            Some(PrefixBreak::HistoryShrunk)
        );
        assert_eq!(
            c.advance(&json!([]), &msgs(&["sys", "x"])),
            Some(PrefixBreak::ToolsChanged)
        );
        assert_eq!(c.breaks(), 3);
    }
}
