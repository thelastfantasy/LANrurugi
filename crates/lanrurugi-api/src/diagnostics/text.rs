//! The feedback text that accompanies a bundle (FR-015), in Chinese and English.
//!
//! Generated on the server from the frozen selection rather than from the UI's i18n strings: the
//! text names version, release and request id, and those are server-side facts. Both languages are
//! produced every time so the administrator picks which to paste.

use lanrurugi_storage::diagnostics::DiagnosticIssue;

use super::bundle::SelectionSummary;

/// One language's title and body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeedbackText {
    pub title: String,
    pub body: String,
}

/// Both languages for one selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bilingual {
    pub zh: FeedbackText,
    pub en: FeedbackText,
}

/// Everything the text states about the environment.
#[derive(Debug, Clone)]
pub struct Facts {
    pub version: String,
    pub release: String,
    pub engine: String,
    pub platform: String,
    pub bundle_name: String,
}

pub fn render(
    facts: &Facts,
    selection: &SelectionSummary,
    top: Option<&DiagnosticIssue>,
) -> Bilingual {
    let range = format!(
        "{} → {}",
        format_instant(selection.from),
        format_instant(selection.to)
    );
    let categories = selection
        .categories
        .iter()
        .map(|c| match c {
            CategoryLabel::ServerLogs => ("服务端日志".to_string(), "server logs".to_string()),
            CategoryLabel::FrontendEvents => {
                ("前端事件".to_string(), "frontend events".to_string())
            }
            CategoryLabel::Environment => ("环境与诊断".to_string(), "environment".to_string()),
            CategoryLabel::Config => ("配置(已脱敏)".to_string(), "redacted config".to_string()),
        })
        .collect::<Vec<_>>();
    let zh_categories = categories
        .iter()
        .map(|c| c.0.as_str())
        .collect::<Vec<_>>()
        .join("、");
    let en_categories = categories
        .iter()
        .map(|c| c.1.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let truncation_zh = if selection.truncations.is_empty() {
        String::new()
    } else {
        format!("\n已截断:{}", selection.truncations.join(";"))
    };
    let truncation_en = if selection.truncations.is_empty() {
        String::new()
    } else {
        format!("\nTruncated: {}", selection.truncations.join("; "))
    };

    let (zh_title, en_title, zh_focus, en_focus) = match top {
        Some(issue) => (
            format!("[诊断] {}", issue.title),
            format!("[diagnostics] {}", issue.title),
            format!(
                "- 主要问题:{} (fingerprint `{}`)\n",
                issue.title,
                issue.fingerprint.as_str()
            ),
            format!(
                "- top issue: {} (fingerprint `{}`)\n",
                issue.title,
                issue.fingerprint.as_str()
            ),
        ),
        None => (
            "[诊断] 反馈包".to_string(),
            "[diagnostics] feedback bundle".to_string(),
            String::new(),
            String::new(),
        ),
    };

    let zh_request = selection
        .request_id
        .as_deref()
        .map(|id| format!("- 请求 id:`{id}`(可在日志里直接搜)\n"))
        .unwrap_or_default();
    let en_request = selection
        .request_id
        .as_deref()
        .map(|id| format!("- request id: `{id}` (searchable in the logs)\n"))
        .unwrap_or_default();

    let zh = FeedbackText {
        title: zh_title,
        body: format!(
            "### 环境\n\
             - 版本:{} / release `{}`\n\
             - 分享引擎版本:{}\n\
             - 平台:{}\n\
             \n\
             ### 复现步骤\n\
             1. \n\
             \n\
             ### 期望\n\
             \n\
             ### 实际\n\
             {zh_focus}{zh_request}\n\
             ### 附件\n\
             - `{}` 覆盖 {range},包含 {zh_categories}({events} 个事件、{lines} 行日志){truncation_zh}\n",
            facts.version,
            facts.release,
            facts.engine,
            facts.platform,
            facts.bundle_name,
            events = selection.events,
            lines = selection.log_lines,
        ),
    };

    let en = FeedbackText {
        title: en_title,
        body: format!(
            "### Environment\n\
             - version {} / release `{}`\n\
             - share engine {}\n\
             - platform {}\n\
             \n\
             ### Steps to reproduce\n\
             1. \n\
             \n\
             ### Expected\n\
             \n\
             ### Actual\n\
             {en_focus}{en_request}\n\
             ### Attachments\n\
             - `{}` covers {range}, containing {en_categories} ({events} events, {lines} log lines){truncation_en}\n",
            facts.version,
            facts.release,
            facts.engine,
            facts.platform,
            facts.bundle_name,
            events = selection.events,
            lines = selection.log_lines,
        ),
    };

    Bilingual { zh, en }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CategoryLabel {
    ServerLogs,
    FrontendEvents,
    Environment,
    Config,
}

/// `2026-10-28 14:12` in UTC, which is what the logs themselves use.
pub fn format_instant(millis: i64) -> String {
    let secs = millis.div_euclid(1000);
    let days = secs.div_euclid(86_400);
    let rem = secs.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02} UTC",
        rem / 3600,
        (rem % 3600) / 60
    )
}

fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_storage::diagnostics::{Fingerprint, IssueState, Level};

    fn summary() -> SelectionSummary {
        SelectionSummary {
            from: 1_793_156_000_000,
            to: 1_793_159_600_000,
            events: 3,
            log_lines: 412,
            categories: vec![
                CategoryLabel::ServerLogs,
                CategoryLabel::FrontendEvents,
                CategoryLabel::Environment,
            ],
            truncations: Vec::new(),
            request_id: Some("7fa3".into()),
        }
    }

    fn facts() -> Facts {
        Facts {
            version: "0.1.0".into(),
            release: "a94a8fe5".into(),
            engine: "v3 (iroh 1.4.1)".into(),
            platform: "linux/x86_64".into(),
            bundle_name: "lrrg-diagnostics-20261028-1412.zip".into(),
        }
    }

    #[test]
    fn both_languages_name_the_facts() {
        let text = render(&facts(), &summary(), None);
        for field in [&text.zh.body, &text.en.body] {
            assert!(field.contains("a94a8fe5"), "{field}");
            assert!(field.contains("v3 (iroh 1.4.1)"), "{field}");
            assert!(
                field.contains("lrrg-diagnostics-20261028-1412.zip"),
                "{field}"
            );
            assert!(field.contains("7fa3"), "{field}");
        }
        assert!(
            text.zh.body.contains("2026-10-28 02:53 UTC"),
            "{}",
            text.zh.body
        );
        assert_ne!(text.zh.title, text.en.title);
    }

    #[test]
    fn the_top_issue_supplies_the_title() {
        let issue = DiagnosticIssue {
            fingerprint: Fingerprint("abc".into()),
            title: "import failed".into(),
            level: Level::Error,
            count: 2,
            first_seen: 0,
            last_seen: 1,
            releases: vec![],
            sources: vec![],
            state: IssueState::Open,
            state_at: None,
            sample_event_id: None,
            request_id: None,
        };
        let text = render(&facts(), &summary(), Some(&issue));
        assert!(text.en.title.contains("import failed"));
        assert!(text.zh.body.contains("abc"));
    }

    #[test]
    fn truncations_are_stated_in_both_languages() {
        let mut selection = summary();
        selection.truncations = vec!["server logs capped at 20 MB".into()];
        let text = render(&facts(), &selection, None);
        assert!(text.en.body.contains("Truncated"));
        assert!(text.zh.body.contains("已截断"));
    }
}
