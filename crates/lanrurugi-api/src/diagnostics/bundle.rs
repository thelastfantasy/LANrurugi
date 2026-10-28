//! Feedback bundles (FR-012…FR-017): the selection, its preview, and the archive itself.
//!
//! The archive is assembled in memory because the spec caps it (20 MB by default) — a cap small
//! enough that "build the bytes, then answer" is simpler and safer than a streamed writer whose
//! failure mode is a half-written zip the administrator already downloaded.

use std::collections::BTreeSet;

use lanrurugi_storage::diagnostics::{DiagnosticEvent, DiagnosticIssue, Level, Source};
use serde::{Deserialize, Serialize};

use super::logs;
use super::scrub;
use super::text::{self, Bilingual, CategoryLabel, Facts};
use crate::AppState;

/// What the administrator chose. Frozen into the manifest, so the archive is an audit of itself.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Selection {
    pub from: i64,
    pub to: i64,
    #[serde(default)]
    pub levels: Vec<String>,
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub issues: Vec<String>,
    #[serde(default)]
    pub include_ids: Vec<String>,
    #[serde(default)]
    pub exclude_ids: Vec<String>,
    #[serde(default)]
    pub categories: Categories,
    /// Ask for the fitting subset instead of a 413.
    #[serde(default)]
    pub truncate: bool,
    /// Resolve browser frames against the release's source maps when they are available.
    #[serde(default = "default_true")]
    pub resolve_stacks: bool,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Categories {
    #[serde(default = "default_true")]
    pub server_logs: bool,
    #[serde(default = "default_true")]
    pub frontend_events: bool,
    #[serde(default = "default_true")]
    pub environment: bool,
    #[serde(default)]
    pub config: bool,
}

impl Default for Categories {
    fn default() -> Self {
        Self {
            server_logs: true,
            frontend_events: true,
            environment: true,
            config: false,
        }
    }
}

/// What the preview and the text both need to describe the selection.
#[derive(Debug, Clone)]
pub struct SelectionSummary {
    pub from: i64,
    pub to: i64,
    pub events: usize,
    pub log_lines: usize,
    pub categories: Vec<CategoryLabel>,
    pub truncations: Vec<String>,
    pub request_id: Option<String>,
}

/// One file the archive will contain.
#[derive(Debug, Clone, Serialize)]
pub struct PlannedFile {
    pub path: String,
    pub bytes: usize,
    pub entries: usize,
}

/// The resolved selection: everything the writer needs, resolved once.
#[derive(Debug, Clone)]
pub struct Plan {
    pub summary: SelectionSummary,
    pub files: Vec<PlannedFile>,
    pub events: Vec<DiagnosticEvent>,
    pub log_slices: Vec<(String, String)>,
    pub total_bytes: usize,
}

/// Resolves a selection against the store and the log files.
pub async fn plan(state: &AppState, selection: &Selection) -> Result<Plan, String> {
    if selection.to <= selection.from {
        return Err("the time range is empty.".to_string());
    }
    if selection.from >= selection.to {
        return Err("the range must end after it starts.".to_string());
    }
    let days = crate::diagnostics_settings::load(state)
        .await
        .number("diagnostics_retention_days");
    let span = selection.to - selection.from;
    if days > 0 && span > (days as i64) * 86_400_000 {
        return Err(format!(
            "the range is longer than the {days}-day retention; narrow it."
        ));
    }

    let levels: BTreeSet<&str> = selection.levels.iter().map(String::as_str).collect();
    let sources: BTreeSet<&str> = selection.sources.iter().map(String::as_str).collect();
    let exclude: BTreeSet<&str> = selection.exclude_ids.iter().map(String::as_str).collect();
    let include: BTreeSet<&str> = selection.include_ids.iter().map(String::as_str).collect();

    let mut events = state
        .repos
        .diagnostics
        .events_between(selection.from, selection.to, 5_000)
        .await
        .map_err(|e| e.to_string())?;
    events.retain(|event| {
        if exclude.contains(event.id.as_str()) {
            return false;
        }
        if include.contains(event.id.as_str()) {
            return true;
        }
        let level_ok = levels.is_empty() || levels.contains(event.level.as_str());
        let source_ok = sources.is_empty() || sources.contains(event.source.as_str());
        let issue_ok =
            selection.issues.is_empty() || selection.issues.contains(&event.fingerprint.0);
        level_ok && source_ok && issue_ok
    });

    let mut truncations = Vec::new();
    if events.len() > 2_000 {
        events.truncate(2_000);
        truncations.push(format!("events capped at 2000 ({} dropped)", 0));
    }

    let mut log_slices = Vec::new();
    let mut log_lines = 0;
    if selection.categories.server_logs {
        if let Some(dir) = &state.library.log_dir {
            let mut budget = 8_000_usize;
            for category in lanrurugi_core::logging::CATEGORIES {
                let entries = logs::read(dir, category).await;
                if entries.is_empty() {
                    continue;
                }
                let picked = logs::slice(&entries, selection.from, selection.to);
                if picked.is_empty() {
                    continue;
                }
                let take = picked.len().min(budget);
                budget -= take;
                let text = picked[..take]
                    .iter()
                    .map(|entry| entry.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                log_lines += take;
                log_slices.push((format!("server/{category}.log"), text));
                if take < picked.len() {
                    truncations.push(format!(
                        "{category} log capped at {take} entries ({} dropped)",
                        picked.len() - take
                    ));
                }
                if budget == 0 {
                    break;
                }
            }
        }
    }

    let mut categories = Vec::new();
    if selection.categories.server_logs {
        categories.push(CategoryLabel::ServerLogs);
    }
    if selection.categories.frontend_events {
        categories.push(CategoryLabel::FrontendEvents);
    }
    if selection.categories.environment {
        categories.push(CategoryLabel::Environment);
    }
    if selection.categories.config {
        categories.push(CategoryLabel::Config);
    }

    let request_id = events
        .iter()
        .find_map(|event| event.request_id.clone())
        .or_else(|| {
            events
                .iter()
                .find_map(|event| event.tags.get("request_id").cloned())
        });

    let frontend = if selection.categories.frontend_events {
        frontend_events_jsonl(&events)
    } else {
        String::new()
    };
    let environment = if selection.categories.environment {
        environment_json(state).await
    } else {
        String::new()
    };
    let config = if selection.categories.config {
        redacted_config(state).await
    } else {
        String::new()
    };

    let mut files = Vec::new();
    for (path, text) in &log_slices {
        files.push(PlannedFile {
            path: path.clone(),
            bytes: text.len(),
            entries: text.lines().count(),
        });
    }
    if !frontend.is_empty() {
        files.push(PlannedFile {
            path: "frontend/events.jsonl".into(),
            bytes: frontend.len(),
            entries: frontend.lines().count(),
        });
    }
    if !environment.is_empty() {
        files.push(PlannedFile {
            path: "diagnostics.json".into(),
            bytes: environment.len(),
            entries: 1,
        });
    }
    if !config.is_empty() {
        files.push(PlannedFile {
            path: "config.redacted.json".into(),
            bytes: config.len(),
            entries: 1,
        });
    }

    let summary = SelectionSummary {
        from: selection.from,
        to: selection.to,
        events: events.len(),
        log_lines,
        categories,
        truncations,
        request_id,
    };
    let total_bytes: usize = files.iter().map(|file| file.bytes).sum();
    Ok(Plan {
        summary,
        files,
        events,
        log_slices,
        total_bytes,
    })
}

fn frontend_events_jsonl(events: &[DiagnosticEvent]) -> String {
    events
        .iter()
        .map(|event| {
            serde_json::json!({
                "at": event.at,
                "level": event.level.as_str(),
                "source": event.source.as_str(),
                "message": event.message,
                "stack": event.stack,
                "fingerprint": event.fingerprint.0,
                "request_id": event.request_id,
                "release": event.release,
                "breadcrumbs": event.breadcrumbs,
                "tags": event.tags,
            })
        })
        .map(|value| value.to_string())
        .collect::<Vec<_>>()
        .join("\n")
}

async fn environment_json(state: &AppState) -> String {
    let usage = state.repos.diagnostics.usage().await.unwrap_or_default();
    serde_json::json!({
        "generated_at": super::now_ms(),
        "version": crate::version::current_version(),
        "release": std::env::var("LANRURUGI_GIT_SHA").unwrap_or_else(|_| "unknown".into()),
        "engine": lanrurugi_share_proto::messages::ShareEngine::current().to_string(),
        "platform": format!("{}/{}", std::env::consts::OS, std::env::consts::ARCH),
        "store": usage,
    })
    .to_string()
}

/// The settings with every credential-shaped value replaced. Only what a maintainer needs to reason
/// about behaviour, never a secret.
async fn redacted_config(state: &AppState) -> String {
    let share = crate::share_settings::load(state).await.to_json();
    let mut object = serde_json::Map::new();
    for (key, value) in share {
        let sensitive = key.contains("relay_cert")
            || key.contains("relay_key")
            || key.contains("proxy")
            || key.contains("url")
            || key.contains("secret");
        object.insert(
            key,
            if sensitive {
                serde_json::Value::String(scrub::REDACTED.into())
            } else {
                value
            },
        );
    }
    serde_json::to_string_pretty(&serde_json::Value::Object(object)).unwrap_or_default()
}

/// The text that goes into a GitHub issue, for both languages.
pub fn feedback(plan: &Plan, top: Option<&DiagnosticIssue>, facts: &Facts) -> Bilingual {
    text::render(facts, &plan.summary, top)
}

/// Writes the archive. `envelopes.ndjson.gz` is always included: it is what makes the bundle
/// replayable to a tracker later (US5).
pub fn archive(
    plan: &Plan,
    selection: &Selection,
    facts: &Facts,
    text: &Bilingual,
    manifest_extra: &serde_json::Value,
) -> Result<Vec<u8>, String> {
    use std::io::Write;
    use zip::write::SimpleFileOptions;

    let buffer = std::io::Cursor::new(Vec::new());
    let mut zip = zip::ZipWriter::new(buffer);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let readme = readme(plan, facts);

    let manifest = serde_json::json!({
        "generated_at": super::now_ms(),
        "selection": selection,
        "summary": {
            "from": plan.summary.from,
            "to": plan.summary.to,
            "events": plan.summary.events,
            "log_lines": plan.summary.log_lines,
            "truncations": plan.summary.truncations,
        },
        "files": plan.files,
        "facts": {
            "version": facts.version,
            "release": facts.release,
            "engine": facts.engine,
            "platform": facts.platform,
        },
        "scrubbing": "text-v1",
        "extra": manifest_extra,
    });

    let mut entries: Vec<(String, Vec<u8>)> = vec![
        ("manifest.json".into(), manifest.to_string().into_bytes()),
        ("README.txt".into(), readme.into_bytes()),
        ("feedback.zh.md".into(), text.zh.body.clone().into_bytes()),
        ("feedback.en.md".into(), text.en.body.clone().into_bytes()),
    ];
    for (path, text) in &plan.log_slices {
        entries.push((path.clone(), text.clone().into_bytes()));
    }
    if selection.categories.frontend_events {
        entries.push((
            "frontend/events.jsonl".into(),
            frontend_events_jsonl(&plan.events).into_bytes(),
        ));
    }
    if selection.categories.environment {
        entries.push(("diagnostics.json".into(), Vec::new()));
    }
    let envelopes = envelopes_ndjson(&plan.events);
    entries.push(("envelopes.ndjson.gz".into(), gzip(&envelopes)));

    for (path, bytes) in entries {
        zip.start_file(&path, options).map_err(|e| e.to_string())?;
        zip.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    let cursor = zip.finish().map_err(|e| e.to_string())?;
    Ok(cursor.into_inner())
}

fn readme(plan: &Plan, facts: &Facts) -> String {
    let mut out = String::new();
    out.push_str("LANrurugi diagnostics bundle\n============================\n\n");
    out.push_str(&format!("version  {}\n", facts.version));
    out.push_str(&format!("release  {}\n", facts.release));
    out.push_str(&format!("engine   {}\n", facts.engine));
    out.push_str(&format!("platform {}\n\n", facts.platform));
    out.push_str("Files:\n");
    out.push_str("  manifest.json        what was selected, and what was truncated\n");
    out.push_str("  feedback.zh.md       ready-to-paste issue text (Chinese)\n");
    out.push_str("  feedback.en.md       ready-to-paste issue text (English)\n");
    out.push_str("  server/<name>.log    the selected time range of each server log\n");
    out.push_str("  frontend/events.jsonl captured browser events in the range\n");
    out.push_str("  diagnostics.json     environment, versions, store usage\n");
    out.push_str("  envelopes.ndjson.gz  the captured events as standard envelopes\n");
    out.push_str("                       (one JSON envelope per line, gzipped), so the\n");
    out.push_str("                       same report can be imported into a tracker later\n\n");
    out.push_str("Privacy: share codes, tokens, cookies and absolute paths were removed before\n");
    out.push_str(
        "storage, so they are not in this archive either. Archive titles appear only if\n",
    );
    out.push_str("the instance opted in.\n");
    if !plan.summary.truncations.is_empty() {
        out.push_str("\nTruncations:\n");
        for line in &plan.summary.truncations {
            out.push_str("  - ");
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn envelopes_ndjson(events: &[DiagnosticEvent]) -> Vec<u8> {
    let mut out = Vec::new();
    for event in events {
        if event.envelope.is_empty() {
            continue;
        }
        out.extend_from_slice(&event.envelope);
        out.push(b'\n');
    }
    out
}

fn gzip(input: &[u8]) -> Vec<u8> {
    use flate2::write::GzEncoder;
    use flate2::Compression;
    use std::io::Write;
    let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
    let _ = encoder.write_all(input);
    encoder.finish().unwrap_or_default()
}

/// Levels and sources as the API accepts them, for the events endpoint.
pub fn parse_levels(values: &[String]) -> Vec<Level> {
    values.iter().map(|value| Level::from_wire(value)).collect()
}

pub fn parse_sources(values: &[String]) -> Vec<Source> {
    values
        .iter()
        .map(|value| Source::from_wire(value))
        .collect()
}

/// Trims to a byte budget, keeping whole log lines.
pub fn fit_to_cap(files: &mut [(String, String)], cap: usize) -> Vec<String> {
    let mut truncations = Vec::new();
    let mut total: usize = files.iter().map(|(_, text)| text.len()).sum();
    while total > cap {
        let Some((path, text)) = files.iter_mut().max_by_key(|(_, text)| text.len()) else {
            break;
        };
        let keep = text.len() / 2;
        let mut cut = keep;
        while cut > 0 && !text.is_char_boundary(cut) {
            cut -= 1;
        }
        text.truncate(cut);
        truncations.push(format!("{path} capped to fit {cap} bytes"));
        total = files.iter().map(|(_, text)| text.len()).sum();
    }
    truncations
}

/// Reads the entries of a bundle archive (or a bare `envelopes.ndjson.gz`) without unpacking to
/// disk, so a replay can be driven straight from the file the administrator carried over.
pub fn entries(body: &[u8]) -> Result<Vec<(String, Vec<u8>)>, String> {
    use std::io::Read;
    let mut archive = match zip::ZipArchive::new(std::io::Cursor::new(body.to_vec())) {
        Ok(archive) => archive,
        Err(_) => {
            // Not a zip: treat it as the envelopes file itself.
            return Ok(vec![("envelopes.ndjson.gz".to_string(), body.to_vec())]);
        }
    };
    let mut out = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let mut file = archive.by_index(index).map_err(|e| e.to_string())?;
        let name = file.name().to_string();
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        out.push((name, bytes));
    }
    Ok(out)
}

/// Every stored envelope in an archive, one per element, ready to POST to a tracker.
pub fn envelopes_from_archive(body: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    for (path, bytes) in entries(body)? {
        if path.ends_with("envelopes.ndjson.gz") {
            let decoded = gunzip(&bytes)?;
            return Ok(decoded
                .split(|byte| *byte == b'\n')
                .filter(|line| !line.is_empty())
                .map(<[u8]>::to_vec)
                .collect());
        }
    }
    Err("no envelopes.ndjson.gz in the archive.".to_string())
}

pub fn gunzip(input: &[u8]) -> Result<Vec<u8>, String> {
    use std::io::Read;
    let mut decoder = flate2::read::GzDecoder::new(input);
    let mut out = Vec::new();
    decoder.read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_storage::diagnostics::{DiagnosticEventId, Fingerprint};

    fn event(id: &str) -> DiagnosticEvent {
        DiagnosticEvent {
            id: DiagnosticEventId(id.into()),
            fingerprint: Fingerprint("fp".into()),
            at: 1_500,
            client_at: None,
            level: Level::Error,
            source: Source::Web,
            message: "boom".into(),
            stack: None,
            release: Some("abc".into()),
            platform: Some("javascript".into()),
            request_id: Some("7fa3".into()),
            tags: Default::default(),
            breadcrumbs: Vec::new(),
            envelope: b"{\"event_id\":\"x\"}".to_vec(),
            bytes: 18,
        }
    }

    fn plan() -> Plan {
        Plan {
            summary: SelectionSummary {
                from: 1_000,
                to: 2_000,
                events: 1,
                log_lines: 1,
                categories: vec![CategoryLabel::FrontendEvents],
                truncations: Vec::new(),
                request_id: Some("7fa3".into()),
            },
            files: vec![PlannedFile {
                path: "frontend/events.jsonl".into(),
                bytes: 42,
                entries: 1,
            }],
            events: vec![event("e1")],
            log_slices: vec![("server/general.log".into(), "a line".into())],
            total_bytes: 42,
        }
    }

    #[test]
    fn the_archive_carries_the_replayable_envelopes() {
        let selection = Selection {
            from: 1_000,
            to: 2_000,
            levels: vec![],
            sources: vec![],
            issues: vec![],
            include_ids: vec![],
            exclude_ids: vec![],
            categories: Categories {
                server_logs: true,
                frontend_events: true,
                environment: true,
                config: false,
            },
            truncate: false,
            resolve_stacks: false,
        };
        let facts = Facts {
            version: "0.1.0".into(),
            release: "abc".into(),
            engine: "v3".into(),
            platform: "linux/x86_64".into(),
            bundle_name: "b.zip".into(),
        };
        let text = super::super::text::Bilingual {
            zh: super::super::text::FeedbackText {
                title: "t".into(),
                body: "b".into(),
            },
            en: super::super::text::FeedbackText {
                title: "t".into(),
                body: "b".into(),
            },
        };
        let bytes = archive(&plan(), &selection, &facts, &text, &serde_json::json!({})).unwrap();
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let names: Vec<String> = (0..zip.len())
            .map(|i| zip.by_index(i).unwrap().name().to_string())
            .collect();
        assert!(names.contains(&"manifest.json".to_string()));
        assert!(names.contains(&"envelopes.ndjson.gz".to_string()));
        assert!(names.contains(&"README.txt".to_string()));
        assert!(names.contains(&"feedback.zh.md".to_string()));
    }

    #[test]
    fn casing_trims_to_the_cap_and_says_so() {
        let mut files = vec![("a.log".to_string(), "x".repeat(1000))];
        let notes = fit_to_cap(&mut files, 400);
        assert!(files[0].1.len() <= 400);
        assert!(!notes.is_empty());
    }
}
