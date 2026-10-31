//! `lanrurugi-api-docgen` — generate the API reference document from the server's own route table.
//!
//! See `specs/011-api-docs-site/contracts/docgen-cli.md` for the command-line contract, and
//! `specs/011-api-docs-site/contracts/openapi-document.md` for what the output contains.
//!
//! Run it from the repository root. All default paths are repository-relative, so the tool behaves
//! identically from `mise run docs-api`, a direct `cargo run`, and a Docker `RUN`.

use std::path::PathBuf;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

use lanrurugi_api_docgen::{access_notes, emit, legacy, merge, policy, scan, sidecar, validate};

/// Generator configuration. Every path is overridable so the tests can point the whole pipeline at
/// a fixture tree instead of the real repository.
#[derive(Debug, Parser)]
#[command(
    name = "lanrurugi-api-docgen",
    about = "Generate the OpenAPI document for LANrurugi's HTTP API from its own route table",
    version
)]
struct Args {
    /// Where to write the generated document.
    #[arg(long, default_value = "docs/public/openapi.json")]
    output: PathBuf,

    /// Root of the route source to scan.
    #[arg(long, default_value = "crates/lanrurugi-api/src")]
    api_src: PathBuf,

    /// The file that mounts the routers. Scanned for mount consistency only.
    #[arg(long, default_value = "crates/lanrurugi-server/src/app.rs")]
    app_src: PathBuf,

    /// The vendored upstream LANraragi OpenAPI document.
    #[arg(long, default_value = "docs/api/legacy-openapi.yaml")]
    legacy: PathBuf,

    /// Directory of hand-written OpenAPI fragments.
    #[arg(long, default_value = "docs/api/sidecar")]
    sidecar: PathBuf,

    /// Per-role behaviour notes that OpenAPI cannot express (see the `access_notes` module).
    #[arg(long, default_value = "docs/api/access-notes.yaml")]
    access_notes: PathBuf,

    /// The route policy the server enforces. Read so each operation's `security` states its real
    /// requirement rather than one blanket claim — see `policy`'s own module docs for why the
    /// difference is not cosmetic.
    #[arg(long, default_value = "crates/lanrurugi-api/policy/route_policy.csv")]
    policy: PathBuf,

    /// Prefix stripped from emitted source locations, so no absolute path reaches the document.
    #[arg(long, default_value = ".")]
    repository: PathBuf,

    /// Validate only: run every gate and the coverage report, write nothing.
    #[arg(long)]
    check: bool,

    /// Suppress the coverage report on success. Never suppresses errors, and never affects a gate.
    #[arg(long)]
    quiet: bool,
}

/// Exit codes are part of the contract: `1` means a gate rejected the document, `2` means the tool
/// could not run at all. CI and `mise` tell those apart.
enum Failure {
    Gate(anyhow::Error),
    Fatal(anyhow::Error),
}

impl From<anyhow::Error> for Failure {
    fn from(error: anyhow::Error) -> Self {
        Failure::Fatal(error)
    }
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(Failure::Gate(error)) => {
            eprintln!("error: {error:#}");
            ExitCode::from(1)
        }
        Err(Failure::Fatal(error)) => {
            eprintln!("error: {error:#}");
            ExitCode::from(2)
        }
    }
}

fn run(args: &Args) -> Result<(), Failure> {
    let repository = args.repository.canonicalize().with_context(|| {
        format!(
            "resolving the repository root `{}` — run this from the repository root, or pass \
                 --repository",
            args.repository.display()
        )
    })?;

    let scanned = scan::scan(
        &args.api_src,
        std::slice::from_ref(&args.app_src),
        &repository,
    )?;
    let mounts = scan::resolve_mounts(&scanned)?;

    let legacy_layer = legacy::load(&args.legacy, &scanned.routes)?;
    let legacy_prefix = legacy_layer.prefix.clone();
    let sidecar_layer = sidecar::load(&args.sidecar, &scanned.routes)?;
    let sidecar_files = sidecar_layer.files_read;

    let fns = scanned.fn_lookup();
    let mut merged = merge::merge(&scanned.routes, legacy_layer, sidecar_layer, &fns)?;
    merged.coverage.unreferenced_factories = mounts.unreferenced_factories.clone();

    let route_policy = policy::Policy::load(&args.policy)?;
    let notes = access_notes::load(&args.access_notes, &scanned.routes)?;

    validate::run(&merged, &route_policy).map_err(Failure::Gate)?;

    validate::report(
        &merged,
        &route_policy,
        &legacy_prefix,
        sidecar_files,
        notes.route_count(),
        args.quiet,
    );

    if args.check {
        if !args.quiet {
            println!("--check: gates passed, nothing written.");
        }
        return Ok(());
    }

    let document = emit::assemble(&merged, &route_policy, &notes, env!("CARGO_PKG_VERSION"));
    let json = emit::to_json(&document);

    if let Some(parent) = args.output.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(&args.output, json)
        .with_context(|| format!("writing {}", args.output.display()))?;
    if !args.quiet {
        println!("wrote {}", args.output.display());
    }
    Ok(())
}
