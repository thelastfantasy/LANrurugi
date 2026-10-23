//! `lanrurugi-server eval-grouping` (`specs/016-tankoubon-grouping-harness`
//! contracts/eval-cli.md): measure the tankoubon-grouping pipeline against the user's own labels
//! before changing it. Read-only for library data and labels.

use std::path::PathBuf;

use lanrurugi_api::tankoubon_grouping::{evaluate, EvalReview, EvaluationReport, GroupingSettings};

#[derive(clap::Args)]
pub struct EvalGroupingArgs {
    #[arg(
        long,
        env = "LANRURUGI_REDIS_URL",
        default_value = "redis://127.0.0.1:6379"
    )]
    pub redis_url: String,

    /// JSON file with the settings to compare against the current ones; omitted fields keep
    /// their current value.
    #[arg(long)]
    pub candidate: Option<PathBuf>,

    /// `off`: local stage only. `cached`: apply cached reviews. `live`: may call the provider for
    /// groups without a cached review.
    #[arg(long, default_value = "off")]
    pub review: String,
}

pub async fn run(args: EvalGroupingArgs) -> anyhow::Result<()> {
    let review = match args.review.as_str() {
        "off" => EvalReview::Off,
        "cached" => EvalReview::Cached,
        "live" => EvalReview::Live,
        other => anyhow::bail!("--review must be off, cached or live, not {other:?}"),
    };
    let candidate: Option<GroupingSettings> = match &args.candidate {
        Some(path) => Some(serde_json::from_str(&std::fs::read_to_string(path)?)?),
        None => None,
    };
    let redis = lanrurugi_storage::redis::RedisDbs::connect(&args.redis_url)?;
    match evaluate(&redis, candidate, review)
        .await
        .map_err(anyhow::Error::msg)?
    {
        None => println!("No grouping labels recorded yet — nothing to measure."),
        Some(report) => print!("{}", render(&report)),
    }
    Ok(())
}

fn pct(v: Option<f64>) -> String {
    v.map(|x| format!("{:.1}%", x * 100.0))
        .unwrap_or_else(|| "—".to_string())
}

/// The report as a plain-text table.
pub fn render(report: &EvaluationReport) -> String {
    let mut out = format!(
        "labels: {} positive, {} negative; skipped archives (missing or no cached vector): {}\n\n",
        report.positive_labels, report.negative_labels, report.skipped_archives
    );
    out.push_str(&format!(
        "{:<10} {:<9} {:>7} {:>10} {:>9} {:>14}\n",
        "variant", "stage", "groups", "precision", "recall", "pairs (tp/all)"
    ));
    for v in &report.variants {
        let mut rows = vec![("local", &v.local)];
        if let Some(p) = &v.pipeline {
            rows.push(("pipeline", p));
        }
        for (stage, m) in rows {
            out.push_str(&format!(
                "{:<10} {:<9} {:>7} {:>10} {:>9} {:>14}\n",
                v.name,
                stage,
                v.groups,
                pct(m.precision),
                pct(m.recall),
                format!("{}/{}", m.true_positive, m.proposed_labelled)
            ));
        }
        if v.live_reviews > 0 {
            out.push_str(&format!(
                "{:<10} provider reviews this run: {}\n",
                v.name, v.live_reviews
            ));
        }
    }
    if let [current, candidate] = report.variants.as_slice() {
        let delta = |a: Option<f64>, b: Option<f64>| match (a, b) {
            (Some(a), Some(b)) => format!("{:+.1}pt", (b - a) * 100.0),
            _ => "—".to_string(),
        };
        out.push_str(&format!(
            "\ncandidate vs current (local): precision {}, recall {}\n",
            delta(current.local.precision, candidate.local.precision),
            delta(current.local.recall, candidate.local.recall)
        ));
        if let (Some(a), Some(b)) = (&current.pipeline, &candidate.pipeline) {
            out.push_str(&format!(
                "candidate vs current (pipeline): precision {}, recall {}\n",
                delta(a.precision, b.precision),
                delta(a.recall, b.recall)
            ));
        }
    }
    out
}
