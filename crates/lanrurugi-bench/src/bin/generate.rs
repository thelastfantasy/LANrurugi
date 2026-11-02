//! `lanrurugi-bench-generate` (T084): standalone CLI wrapping
//! `lanrurugi_bench::synthetic_library::generate`, so the synthetic test library can be produced
//! once and pointed at by both systems in the cross-system comparison, without needing the full
//! `compare` binary if a caller just wants the library on disk.

use std::path::PathBuf;

use clap::Parser;
use lanrurugi_bench::synthetic_library::{generate, GenerateConfig};

#[derive(Parser)]
struct Args {
    #[arg(long)]
    output_dir: PathBuf,

    /// SC-008's target scale; pass a smaller value for a fast local run.
    #[arg(long, default_value_t = 100_000)]
    archive_count: usize,

    #[arg(long, default_value_t = 20)]
    pages_per_archive: usize,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let args = Args::parse();

    let summary = generate(&GenerateConfig {
        output_dir: args.output_dir,
        archive_count: args.archive_count,
        pages_per_archive: args.pages_per_archive,
    })?;

    println!("{}", serde_json::to_string_pretty(&summary)?);
    Ok(())
}
