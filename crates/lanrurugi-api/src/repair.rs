//! Bounded self-repair (`specs/014-ai-harness-hardening` US5): a model answer is cleaned, parsed
//! into its expected shape and checked by a deterministic validator. A parse failure or a defect gets
//! exactly one more model call carrying that specific problem; a second failure ends the action as
//! `Malformed`/`Rejected`. The code is straight-line — at most two calls, however much time or budget
//! remains (FR-020/FR-021).

use std::future::Future;

use lanrurugi_llm::{CallSpec, LlmClient, LlmError};
use serde::de::DeserializeOwned;

/// A checked answer and whether it took the repair call.
#[derive(Debug, Clone, PartialEq)]
pub struct Checked<T> {
    pub value: T,
    pub repaired: bool,
}

/// Parses model output as `T`. Format cleaning beyond the seam's fence stripping: when the text
/// has prose around one JSON value, the outermost `{…}`/`[…]` span is tried before giving up.
pub fn parse_json<T: DeserializeOwned>(text: &str) -> Result<T, String> {
    let whole = match serde_json::from_str::<T>(text.trim()) {
        Ok(v) => return Ok(v),
        Err(e) => e.to_string(),
    };
    for (open, close) in [('{', '}'), ('[', ']')] {
        if let (Some(start), Some(end)) = (text.find(open), text.rfind(close)) {
            if start < end {
                if let Ok(v) = serde_json::from_str::<T>(&text[start..=end]) {
                    return Ok(v);
                }
            }
        }
    }
    Err(whole)
}

/// Runs `ask(user)`, parses and validates; on a problem, runs `ask` once more with the repair
/// follow-up naming it. `ask` returns the raw answer text.
pub async fn checked<T, F, Fut>(
    user: &str,
    ask: F,
    validate: impl Fn(&T) -> Result<(), String>,
) -> Result<Checked<T>, LlmError>
where
    T: DeserializeOwned,
    F: Fn(String) -> Fut,
    Fut: Future<Output = Result<String, LlmError>>,
{
    let first = ask(user.to_string()).await?;
    let defect = match parse_json::<T>(&first) {
        Ok(value) => match validate(&value) {
            Ok(()) => {
                return Ok(Checked {
                    value,
                    repaired: false,
                })
            }
            Err(defect) => defect,
        },
        Err(parse_error) => crate::llm_prompts::unparseable_defect(&parse_error),
    };
    tracing::info!(defect = %defect, "model output failed its check; issuing one repair");
    let second = ask(crate::llm_prompts::repair_follow_up(user, &first, &defect)).await?;
    let value = parse_json::<T>(&second).map_err(|_| LlmError::unparseable(&second))?;
    validate(&value).map_err(LlmError::rejected)?;
    Ok(Checked {
        value,
        repaired: true,
    })
}

/// [`checked`] over one JSON-mode chat call — the default for every structured model answer.
pub async fn json_chat_checked<T: DeserializeOwned>(
    llm: &impl LlmClient,
    spec: &CallSpec,
    system: &str,
    user: &str,
    temperature: f32,
    max_tokens: u32,
    validate: impl Fn(&T) -> Result<(), String>,
) -> Result<Checked<T>, LlmError> {
    checked(
        user,
        |user| async move { llm.chat(spec, system, &user, temperature, max_tokens).await },
        validate,
    )
    .await
}

/// No check beyond the shape itself — for answers whose only contract is their type.
pub fn shape_only<T>(_: &T) -> Result<(), String> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Debug, serde::Deserialize, PartialEq)]
    struct N {
        n: u32,
    }

    fn even(v: &N) -> Result<(), String> {
        if v.n.is_multiple_of(2) {
            Ok(())
        } else {
            Err(format!("{} is odd", v.n))
        }
    }

    /// A scripted model: answers in order, recording every prompt it was sent.
    fn script(answers: &[&str]) -> (Mutex<Vec<String>>, Mutex<Vec<String>>) {
        (
            Mutex::new(answers.iter().rev().map(|s| s.to_string()).collect()),
            Mutex::new(Vec::new()),
        )
    }

    async fn run(answers: &[&str]) -> (Result<Checked<N>, LlmError>, Vec<String>) {
        let (queue, seen) = script(answers);
        let out = checked(
            "task",
            |user| {
                seen.lock().unwrap().push(user);
                let next = queue.lock().unwrap().pop().unwrap_or_default();
                async move { Ok(next) }
            },
            even,
        )
        .await;
        (out, seen.into_inner().unwrap())
    }

    #[tokio::test]
    async fn a_valid_first_answer_is_one_call() {
        let (out, seen) = run(&[r#"{"n":2}"#]).await;
        assert_eq!(
            out.unwrap(),
            Checked {
                value: N { n: 2 },
                repaired: false
            }
        );
        assert_eq!(seen.len(), 1);
    }

    #[tokio::test]
    async fn prose_around_the_json_is_cleaned_without_a_repair() {
        let (out, seen) = run(&["Here you go: {\"n\":4} hope it helps"]).await;
        assert_eq!(out.unwrap().value, N { n: 4 });
        assert_eq!(seen.len(), 1);
    }

    #[tokio::test]
    async fn the_repair_names_the_validator_defect() {
        let (out, seen) = run(&[r#"{"n":3}"#, r#"{"n":6}"#]).await;
        assert_eq!(
            out.unwrap(),
            Checked {
                value: N { n: 6 },
                repaired: true
            }
        );
        assert_eq!(seen.len(), 2);
        assert!(seen[1].contains("3 is odd"), "{}", seen[1]);
        // Same prefix as the first request, so the provider's prompt cache covers it.
        assert!(seen[1].starts_with(&seen[0]));
    }

    #[tokio::test]
    async fn a_wrong_shape_gets_one_repair_too() {
        let (out, seen) = run(&[r#"{"count":2}"#, r#"{"n":8}"#]).await;
        assert_eq!(out.unwrap().value, N { n: 8 });
        assert!(seen[1].contains("missing field"), "{}", seen[1]);
    }

    #[tokio::test]
    async fn a_second_failure_ends_after_exactly_two_calls() {
        let (out, seen) = run(&[r#"{"n":3}"#, r#"{"n":5}"#, r#"{"n":8}"#]).await;
        let err = out.unwrap_err();
        assert_eq!(err.class(), lanrurugi_llm::LlmErrorClass::Rejected);
        assert_eq!(err.message(), "5 is odd");
        assert_eq!(seen.len(), 2);

        let (out, seen) = run(&["not json", "still not json"]).await;
        assert_eq!(
            out.unwrap_err().class(),
            lanrurugi_llm::LlmErrorClass::Malformed
        );
        assert_eq!(seen.len(), 2);
    }
}
