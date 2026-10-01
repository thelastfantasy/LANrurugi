//! `lanrurugi-bench-compare`: standalone CLI for the cross-system comparison harness (T086),
//! usable independently of a running `lanrurugi serve` instance (unlike `POST /bench/run`, T089,
//! which is for triggering it from inside the app itself). Expects both a legacy LANraragi
//! instance and a `lanrurugi serve` instance already running and pointed at the same synthetic
//! library copy (produced by `lanrurugi-bench-generate`).

use clap::Parser;
use lanrurugi_bench::compare::{run_full_comparison, CompareConfig, SystemEndpoint};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    legacy_url: String,
    #[arg(long)]
    legacy_api_key: Option<String>,

    #[arg(long)]
    new_url: String,
    #[arg(long)]
    new_api_key: Option<String>,

    #[arg(long)]
    archive_count: u64,
    #[arg(long, default_value_t = 0)]
    total_size_bytes: u64,

    #[arg(long, default_value = "unspecified")]
    hardware_description: String,

    #[arg(long, default_value = "Synthetic")]
    title_needle: String,

    #[arg(long, default_value_t = 20)]
    interactive_load_iterations: usize,

    #[arg(long, default_value_t = 250)]
    poll_interval_ms: u64,

    #[arg(long, default_value_t = 1800)]
    operation_timeout_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let config = CompareConfig {
        legacy: SystemEndpoint {
            base_url: args.legacy_url,
            api_key: args.legacy_api_key,
        },
        new: SystemEndpoint {
            base_url: args.new_url,
            api_key: args.new_api_key,
        },
        archive_count: args.archive_count,
        total_size_bytes: args.total_size_bytes,
        hardware_description: args.hardware_description,
        poll_interval: std::time::Duration::from_millis(args.poll_interval_ms),
        operation_timeout: std::time::Duration::from_secs(args.operation_timeout_secs),
        title_needle: args.title_needle,
        interactive_load_iterations: args.interactive_load_iterations,
    };

    let report = run_full_comparison(&config).await?;
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}
