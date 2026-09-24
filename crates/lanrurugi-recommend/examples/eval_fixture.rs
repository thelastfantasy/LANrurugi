//! Quantified evaluation of the embedding-based series-recognition prefilter against the same
//! 60-real-title fixture `recommend.rs`'s own binary pass/fail acceptance test uses (see that
//! module's `tests::load_fixture_with_volumes` — loaded from a path supplied via
//! `LANRURUGI_TEST_FIXTURE_SERIES_TITLES_PATH`, not checked into source, since it's real,
//! copyrighted work titles) — a scoring companion to that pass/fail test, built to let two
//! different model files be compared by an actual number rather than "did the pass/fail test
//! pass" (issue #70's smaller-model-for-cheaper-memory question needs a real accuracy delta to
//! judge, not a guess).
//!
//! For every fixture title whose `series_group` has ≥2 members (a standalone has no same-series
//! candidate to be graded against, same exclusion the acceptance test applies), embeds it against
//! every other fixture title and reports:
//! - **miss rate**: the fraction of titles where no same-series candidate appears anywhere in the
//!   candidate pool at all (a total prefilter failure for that title — the LLM rerank downstream
//!   would never see the right answer).
//! - **top-1 same-series rate**: the fraction where the #1-ranked candidate is same-series (the
//!   strictest bar; the acceptance test doesn't require this, but a smaller model regressing here
//!   is the leading indicator to watch for).
//! - **mean rank of best same-series candidate**: among the titles that did have a hit, how far
//!   down the ranked list the *best* same-series candidate landed on average (1.0 = always
//!   ranked first for a hit).
//!
//! Run with:
//! `cargo run -p lanrurugi-recommend --example eval_fixture -- <model.onnx> <tokenizer.json> [prefix]`
//!
//! `[prefix]` is optional and prepended to every title before embedding (e.g. `"query: "` — the
//! e5 model family's own documented recommendation is to prefix inputs with `"query: "` or
//! `"passage: "` depending on symmetric-vs-asymmetric search; this recommender's use case is
//! symmetric similarity between titles, so `"query: "` on both sides is the applicable form).
//! Lets this script A/B a model with vs. without its recommended prefix convention without
//! touching the production embedding call sites (`recommend.rs`'s `recommend()` deliberately
//! embeds titles verbatim — no model-specific prefix logic).

use lanrurugi_recommend::embedding::Embedder;
use lanrurugi_recommend::recommend::{normalize_title, recommend, ArchiveMeta};
use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 {
        eprintln!("usage: eval_fixture <model.onnx> <tokenizer.json> [prefix]");
        std::process::exit(1);
    }
    let model_path = Path::new(&args[0]);
    let tokenizer_path = Path::new(&args[1]);
    let prefix = args.get(2).cloned().unwrap_or_default();
    if !prefix.is_empty() {
        println!("Using prefix: {prefix:?}");
    }

    let Ok(fixture_path) = std::env::var("LANRURUGI_TEST_FIXTURE_SERIES_TITLES_PATH") else {
        eprintln!(
            "LANRURUGI_TEST_FIXTURE_SERIES_TITLES_PATH not set — see .env.example \
             (copy to .env.local and fill it in, or export it directly for this run)"
        );
        std::process::exit(1);
    };
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&fixture_path).unwrap()).unwrap();
    let fixture: Vec<(String, String)> = data["archives"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| {
            (
                a["series_group"].as_str().unwrap().to_string(),
                a["title"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    assert!(fixture.len() >= 60, "fixture should have 60+ entries");

    let group_sizes: std::collections::HashMap<&String, usize> = {
        let mut m = std::collections::HashMap::new();
        for (g, _) in &fixture {
            *m.entry(g).or_insert(0) += 1;
        }
        m
    };

    println!("Loading model {} ...", model_path.display());
    let load_start = std::time::Instant::now();
    let embedder =
        Embedder::load(model_path, tokenizer_path, 4).expect("model must load — check paths");
    println!("Model loaded in {:.2}s", load_start.elapsed().as_secs_f64());

    let mut checked = 0usize;
    let mut misses = 0usize;
    let mut top1_hits = 0usize;
    let mut rank_sum_for_hits = 0usize;
    let mut hit_count = 0usize;
    let mut miss_examples: Vec<String> = Vec::new();

    let embed_start = std::time::Instant::now();
    for (idx, (group, title)) in fixture.iter().enumerate() {
        if group_sizes.get(group).copied().unwrap_or(0) < 2 {
            continue;
        }
        let others: Vec<ArchiveMeta> = fixture
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != idx)
            .map(|(j, (_, t))| ArchiveMeta {
                id: format!("id{j}"),
                title: format!("{prefix}{t}"),
                tags: String::new(),
                pagecount: 0,
            })
            .collect();

        let normalized_title = format!("{prefix}{}", normalize_title(title));
        let pool = recommend(&embedder, "current", &normalized_title, "", &others, 100)
            .expect("recommend must not fail");
        checked += 1;

        if pool.is_empty() {
            misses += 1;
            miss_examples.push(title.clone());
            continue;
        }

        let mut best_rank: Option<usize> = None;
        for (rank, r) in pool.iter().enumerate() {
            let r_title_unprefixed = r.title.strip_prefix(&prefix).unwrap_or(&r.title);
            let is_same_series = fixture
                .iter()
                .any(|(g, t)| t == r_title_unprefixed && g == group);
            if is_same_series {
                best_rank = Some(rank + 1); // 1-indexed
                break;
            }
        }
        match best_rank {
            Some(rank) => {
                hit_count += 1;
                rank_sum_for_hits += rank;
                if rank == 1 {
                    top1_hits += 1;
                }
            }
            None => {
                misses += 1;
                miss_examples.push(title.clone());
            }
        }
    }
    let embed_elapsed = embed_start.elapsed();

    println!();
    println!("=== Results ({} titles graded) ===", checked);
    println!(
        "Miss rate (no same-series candidate anywhere in pool): {}/{} = {:.1}%",
        misses,
        checked,
        100.0 * misses as f64 / checked as f64
    );
    println!(
        "Top-1 same-series rate: {}/{} = {:.1}%",
        top1_hits,
        checked,
        100.0 * top1_hits as f64 / checked as f64
    );
    if hit_count > 0 {
        println!(
            "Mean rank of best same-series candidate (among hits): {:.2}",
            rank_sum_for_hits as f64 / hit_count as f64
        );
    }
    println!(
        "Total embedding+ranking time for {} titles: {:.2}s ({:.1}ms/title)",
        checked,
        embed_elapsed.as_secs_f64(),
        embed_elapsed.as_secs_f64() * 1000.0 / checked as f64
    );
    if !miss_examples.is_empty() {
        println!();
        println!("Miss examples (first 10):");
        for t in miss_examples.iter().take(10) {
            println!("  - {t}");
        }
    }
}
