//! `lanrurugi-plugin-converter convert <input.pm> [-o output.ts]` — best-effort Perl→TS plugin
//! converter, or `lanrurugi-plugin-converter serve` for the same conversion behind a minimal web
//! UI. Both need `perl` + PPI/JSON::PP on `PATH` for the body-conversion pass (see
//! `Dockerfile.build` at the repo root, or `cargo run` from inside a container built from it);
//! the `plugin_info` metadata pass alone has no such requirement.

use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "lanrurugi-plugin-converter", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Convert a single `.pm` file, printing the result to stdout (or `-o`).
    Convert(ConvertArgs),
    /// Run the web UI (paste `.pm` source, get converted TS back) — same conversion logic, no
    /// terminal needed. Meant to run inside the same container as `convert` (`Dockerfile.build`).
    Serve(ServeArgs),
}

#[derive(clap::Args)]
struct ConvertArgs {
    /// Path to the legacy `.pm` plugin file to convert.
    input: PathBuf,

    /// Where to write the converted `.ts` file. Defaults to stdout.
    #[arg(short, long)]
    output: Option<PathBuf>,
}

#[derive(clap::Args)]
struct ServeArgs {
    /// Address to bind the web UI to.
    #[arg(
        long,
        env = "LANRURUGI_PLUGIN_CONVERTER_BIND",
        default_value = "0.0.0.0:8080"
    )]
    bind: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Command::Convert(args) => convert(args),
        Command::Serve(args) => serve(args).await,
    }
}

fn convert(args: ConvertArgs) -> anyhow::Result<()> {
    let result = lanrurugi_plugin_converter::convert_file(&args.input)?;

    match args.output {
        Some(path) => {
            std::fs::write(&path, &result.ts)?;
            eprintln!("Wrote {}", path.display());
        }
        None => println!("{}", result.ts),
    }

    if !result.warnings.is_empty() {
        eprintln!("\n{} conversion warning(s):", result.warnings.len());
        for warning in &result.warnings {
            eprintln!("  - {warning}");
        }
        eprintln!("\nReview the // TODO(perl-convert) markers and the original-Perl comments in the output before trusting it.");
    }

    Ok(())
}

async fn serve(args: ServeArgs) -> anyhow::Result<()> {
    let app = lanrurugi_plugin_converter::web::router();
    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    eprintln!(
        "lanrurugi-plugin-converter web UI listening on http://{}",
        args.bind
    );
    axum::serve(listener, app).await?;
    Ok(())
}
