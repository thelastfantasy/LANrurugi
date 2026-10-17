//! Natural-language subscription-condition generation.
//!
//! The model is never trusted to emit `Condition` directly: it emits a small, intentionally
//! constrained DSL (`all` / `any` / `not` / rule), and the deterministic parser below turns that
//! into the real condition tree. That keeps model mistakes out of the persisted value and gives
//! every operator/value a single validation point.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Deserialize;

use lanrurugi_llm::LlmClient;
use lanrurugi_storage::subscriptions::{Condition, FieldOperator, FieldRule, RuleValue};

use crate::common::error;
use crate::AppState;

#[derive(Debug, Deserialize)]
pub(crate) struct AiConditionRequest {
    pub prompt: String,
    /// Candidate fields this source can actually offer. The parser rejects any rule naming a field
    /// outside this list, so the model cannot silently invent a filter the source will never carry.
    #[serde(default)]
    pub fields: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub(crate) struct ParseConditionRequest {
    /// The compact DSL, as JSON text. Reparsed here rather than in the browser so pasted syntax
    /// goes through exactly the same validation as model output.
    pub dsl: String,
    #[serde(default)]
    pub fields: Vec<String>,
}

/// The model-facing DSL. Deliberately JSON rather than free-form text: the same shape can be
/// validated by serde before the semantic parser sees it, and the model does not have to remember
/// exact `kind`/`child`/`children` spellings.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum Dsl {
    All {
        all: Vec<Dsl>,
    },
    Any {
        any: Vec<Dsl>,
    },
    Not {
        not: Box<Dsl>,
    },
    Rule {
        field: String,
        operator: String,
        #[serde(default)]
        value: Option<RuleValue>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Date,
    Number,
    Text,
    List,
}

const LIST_NAMESPACES: &[&str] = &[
    "language",
    "artist",
    "group",
    "parody",
    "character",
    "female",
    "male",
    "other",
];

fn kind_of(field: &str) -> Kind {
    match field {
        "posted_at" => Kind::Date,
        "rating" | "pages" => Kind::Number,
        "title" | "category" | "uploader" => Kind::Text,
        "tags" => Kind::List,
        other if LIST_NAMESPACES.contains(&other) => Kind::List,
        _ => Kind::Text,
    }
}

fn allowed_operators(kind: Kind) -> &'static [FieldOperator] {
    match kind {
        Kind::Date => &[FieldOperator::OlderThan, FieldOperator::NewerThan],
        Kind::Number => &[FieldOperator::Gte, FieldOperator::Lte, FieldOperator::Eq],
        Kind::Text => &[
            FieldOperator::Equals,
            FieldOperator::Contains,
            FieldOperator::In,
            FieldOperator::NotIn,
        ],
        Kind::List => &[
            FieldOperator::IncludesAll,
            FieldOperator::IncludesNone,
            FieldOperator::IsEmpty,
            FieldOperator::IsNotEmpty,
        ],
    }
}

fn parse_operator(raw: &str, kind: Kind) -> Result<FieldOperator, String> {
    let operator: FieldOperator =
        serde_json::from_value(serde_json::Value::String(raw.to_string()))
            .map_err(|_| format!("unknown operator {raw:?}"))?;
    if !allowed_operators(kind).contains(&operator) {
        return Err(format!(
            "operator {raw:?} is not valid for a {kind:?} field"
        ));
    }
    Ok(operator)
}

fn parse_duration(raw: &RuleValue) -> Result<f64, String> {
    match raw {
        RuleValue::Number(seconds) => Ok(*seconds),
        RuleValue::Text(text) => {
            let text = text.trim().to_ascii_lowercase();
            let (number, suffix) = text
                .trim_end_matches(|c: char| c.is_ascii_alphabetic())
                .parse::<f64>()
                .map(|n| {
                    (
                        n,
                        text.trim_start_matches(|c: char| c.is_ascii_digit() || c == '.'),
                    )
                })
                .map_err(|_| format!("invalid duration {text:?}"))?;
            let multiplier = match suffix {
                "s" | "sec" | "secs" | "second" | "seconds" => 1.0,
                "m" | "min" | "mins" | "minute" | "minutes" => 60.0,
                "h" | "hr" | "hrs" | "hour" | "hours" => 3600.0,
                "d" | "day" | "days" => 86400.0,
                "w" | "week" | "weeks" => 7.0 * 86400.0,
                "" => 1.0,
                _ => return Err(format!("invalid duration unit in {text:?}")),
            };
            Ok(number * multiplier)
        }
        other => Err(format!(
            "duration must be a number of seconds or a string like \"3h\"; got {other:?}"
        )),
    }
}

fn parse_number(raw: &RuleValue) -> Result<f64, String> {
    match raw {
        RuleValue::Number(n) => Ok(*n),
        RuleValue::Text(s) => s
            .trim()
            .parse::<f64>()
            .map_err(|_| format!("expected a number, got {s:?}")),
        other => Err(format!("expected a number, got {other:?}")),
    }
}

fn parse_text(raw: &RuleValue) -> Result<String, String> {
    match raw {
        RuleValue::Text(s) => Ok(s.clone()),
        other => Err(format!("expected a text value, got {other:?}")),
    }
}

fn parse_list(raw: &RuleValue) -> Result<Vec<String>, String> {
    match raw {
        RuleValue::List(values) => {
            let values: Vec<String> = values
                .iter()
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
            if values.is_empty() {
                Err("expected at least one list value".into())
            } else {
                Ok(values)
            }
        }
        RuleValue::Text(text) => {
            let values: Vec<String> = text
                .split(',')
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
                .collect();
            if values.is_empty() {
                Err("expected at least one list value".into())
            } else {
                Ok(values)
            }
        }
        other => Err(format!("expected a list of values, got {other:?}")),
    }
}

fn parse_rule(
    field: String,
    operator: String,
    value: Option<RuleValue>,
    fields: &[String],
) -> Result<Condition, String> {
    if !fields.is_empty() && !fields.iter().any(|f| f == &field) {
        return Err(format!("field {field:?} is not available for this source"));
    }
    let kind = kind_of(&field);
    let operator = parse_operator(&operator, kind)?;
    let value = match kind {
        Kind::Date => {
            let raw =
                value.ok_or_else(|| format!("operator {operator:?} needs a duration value"))?;
            RuleValue::Number(parse_duration(&raw)?)
        }
        Kind::Number => {
            let raw =
                value.ok_or_else(|| format!("operator {operator:?} needs a numeric value"))?;
            RuleValue::Number(parse_number(&raw)?)
        }
        Kind::Text => match operator {
            FieldOperator::In | FieldOperator::NotIn => {
                let raw =
                    value.ok_or_else(|| format!("operator {operator:?} needs a list value"))?;
                RuleValue::List(parse_list(&raw)?)
            }
            _ => {
                let raw =
                    value.ok_or_else(|| format!("operator {operator:?} needs a text value"))?;
                RuleValue::Text(parse_text(&raw)?)
            }
        },
        Kind::List => match operator {
            FieldOperator::IsEmpty | FieldOperator::IsNotEmpty => RuleValue::List(Vec::new()),
            _ => {
                let raw =
                    value.ok_or_else(|| format!("operator {operator:?} needs a list value"))?;
                RuleValue::List(parse_list(&raw)?)
            }
        },
    };
    Ok(Condition::Rule(FieldRule {
        field,
        operator,
        value,
    }))
}

fn condition_from_dsl(node: Dsl, fields: &[String]) -> Result<Condition, String> {
    match node {
        Dsl::All { all } => {
            if all.is_empty() {
                return Err("an empty `all` group is not valid".into());
            }
            let children = all
                .into_iter()
                .map(|child| condition_from_dsl(child, fields))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Condition::All { children })
        }
        Dsl::Any { any } => {
            if any.is_empty() {
                return Err("an empty `any` group is not valid".into());
            }
            let children = any
                .into_iter()
                .map(|child| condition_from_dsl(child, fields))
                .collect::<Result<Vec<_>, _>>()?;
            Ok(Condition::Any { children })
        }
        Dsl::Not { not } => Ok(Condition::Not {
            child: Box::new(condition_from_dsl(*not, fields)?),
        }),
        Dsl::Rule {
            field,
            operator,
            value,
        } => parse_rule(field, operator, value, fields),
    }
}

pub(crate) async fn parse(Json(body): Json<ParseConditionRequest>) -> Response {
    let dsl = match serde_json::from_str::<Dsl>(&body.dsl) {
        Ok(dsl) => dsl,
        Err(e) => {
            return error(
                StatusCode::UNPROCESSABLE_ENTITY,
                "parse_condition",
                format!("invalid condition syntax: {e}"),
            );
        }
    };
    match condition_from_dsl(dsl, &body.fields) {
        Ok(condition) => axum::Json(serde_json::json!({ "condition": condition })).into_response(),
        Err(message) => error(StatusCode::UNPROCESSABLE_ENTITY, "parse_condition", message),
    }
}

pub(crate) async fn generate(
    State(state): State<AppState>,
    Json(body): Json<AiConditionRequest>,
) -> Response {
    let prompt = body.prompt.trim().to_string();
    if prompt.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "ai_condition",
            "Describe the condition you want before generating.",
        );
    }
    if body.fields.is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "ai_condition",
            "No candidate fields are available for this source.",
        );
    }

    let system = crate::llm_prompts::subscription_condition_system(&body.fields);
    let dsl = match state
        .redis
        .config
        .json_chat::<Dsl>(&system, &prompt, 0.1, 2000)
        .await
    {
        Ok(dsl) => dsl,
        Err(message) => {
            return error(StatusCode::BAD_GATEWAY, "ai_condition", message);
        }
    };

    match condition_from_dsl(dsl, &body.fields) {
        Ok(condition) => axum::Json(serde_json::json!({ "condition": condition })).into_response(),
        Err(message) => error(StatusCode::UNPROCESSABLE_ENTITY, "ai_condition", message),
    }
}
