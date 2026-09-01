//! crossfoot: Crossfoot evidence collection CLI.
//!
//! Read-only by construction. The RPC layer issues eth_chainId, eth_call,
//! eth_getCode, eth_getBlockByNumber and eth_getLogs and nothing else; there
//! is no signing key, no eth_sendTransaction and no eth_sendRawTransaction
//! anywhere in this binary.

mod abi;
mod bundle;
mod cache;
#[cfg(test)]
mod live_tests;
mod model;
mod mtbill;
mod render;
mod rpc;
mod run_mtbill;
mod run_svzchf;
mod svzchf;
mod util;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Args, Parser, Subcommand};

use cache::Cache;
use rpc::{
    Client, DEFAULT_ARCHIVE_ENDPOINT, DEFAULT_LATEST_ENDPOINT, DEFAULT_LOG_HISTORY_ENDPOINT,
};
use svzchf::LogSource;

#[derive(Parser)]
#[command(
    name = "crossfoot",
    version,
    about = "Pinned block evidence bundles for third party on chain products",
    long_about = None
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Fetch the raw inputs for a target and write an evidence bundle.
    Fetch {
        #[command(subcommand)]
        target: Target,
    },
    /// Recompute a target over a pinned window and write a verdict.
    Run {
        #[command(subcommand)]
        target: RunTarget,
    },
    /// Render a static read-only page over the evidence bundles.
    Render {
        /// Directory the pages are written to.
        #[arg(long, default_value = "site")]
        out: PathBuf,

        /// Directory holding the evidence bundles. Bundles without a
        /// result.json are skipped.
        #[arg(long, default_value = "bundles")]
        bundles: PathBuf,
    },
    /// Print keccak256 of each signature, as a full 32 byte hash (the event
    /// topic0) and as the leading 4 bytes (the function selector). With no
    /// argument, prints the signatures this tool uses.
    Selectors {
        /// Signatures to hash, for example "RateChanged(uint24)".
        signatures: Vec<String>,
    },
}

#[derive(Subcommand)]
enum RunTarget {
    /// Frankencoin savings vault svZCHF.
    Svzchf(RunOpts),
    /// Midas mTBILL consistency bundle.
    Mtbill(RunOpts),
}

#[derive(Args)]
struct RunOpts {
    /// Start of the pinned window. The model is seeded from chain state here.
    #[arg(long)]
    baseline_block: u64,

    /// End of the pinned window. The model is compared against chain state here.
    #[arg(long)]
    block: u64,

    /// Root of the workspace. Cache and bundles live under it.
    #[arg(long, default_value = ".")]
    verify_root: PathBuf,

    /// JSON-RPC endpoints, tried in order.
    #[arg(long = "endpoint")]
    endpoints: Vec<String>,

    /// Log history endpoints, tried in order.
    #[arg(long = "log-endpoint")]
    log_endpoints: Vec<String>,

    /// Serve every read from the cache and fail on a miss.
    #[arg(long)]
    offline: bool,

    /// Wait this many milliseconds before each network call.
    #[arg(long, default_value_t = 0)]
    rpc_delay_ms: u64,
}

#[derive(Subcommand)]
enum Target {
    /// Frankencoin savings vault svZCHF and the savings module it uses.
    Svzchf(FetchOpts),
}

#[derive(Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum LogSourceArg {
    Blockscout,
    Rpc,
    None,
}

#[derive(Args)]
struct FetchOpts {
    /// Block number every state read is pinned to.
    #[arg(long)]
    block: u64,

    /// Earlier block: adds the exchange rate read at that block and starts the
    /// log sweep there instead of at the module's deployment block.
    #[arg(long)]
    baseline_block: Option<u64>,

    /// Root of the workspace. Cache and bundles live under it.
    #[arg(long, default_value = ".")]
    verify_root: PathBuf,

    /// JSON-RPC endpoints, tried in order. Repeat to override the defaults.
    #[arg(long = "endpoint")]
    endpoints: Vec<String>,

    /// Log history endpoints, tried in order. Repeat to override the default.
    #[arg(long = "log-endpoint")]
    log_endpoints: Vec<String>,

    /// Serve every read from the cache and fail on a miss. Proves a run made
    /// no network call.
    #[arg(long)]
    offline: bool,

    /// Where the event history comes from. "blockscout" is one keyless
    /// request for the whole rate path; "rpc" is the chunked eth_getLogs
    /// sweep, which free tier endpoints do not sustain over a full history;
    /// "none" fetches no history.
    #[arg(long, value_enum, default_value_t = LogSourceArg::Blockscout)]
    log_source: LogSourceArg,

    /// Also fetch the complete unfiltered event history, windowed. Off by
    /// default: the recompute needs only the rate path.
    #[arg(long)]
    full_log_history: bool,

    /// Shorthand for --log-source none.
    #[arg(long)]
    skip_logs: bool,

    /// Stop the log sweep after this many chunks. Records a truncation
    /// finding in the bundle when it bites.
    #[arg(long)]
    max_log_chunks: Option<usize>,

    /// Starting block span for each eth_getLogs request. Capped at 10000,
    /// the largest span eth.drpc.org serves on its free plan.
    #[arg(long, default_value_t = 10_000)]
    log_chunk: u64,

    /// Wait this many milliseconds before each network call. Free tier
    /// quotas are the binding constraint on a full sweep, so pacing is
    /// cheaper than backing off after a refusal.
    #[arg(long, default_value_t = 0)]
    rpc_delay_ms: u64,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Render { out, bundles } => match render::render(&bundles, &out) {
            Ok(outcome) => {
                println!("out             {}", outcome.out_dir.display());
                println!("runs rendered   {}", outcome.rendered);
                println!("skipped         {} (no result.json)", outcome.skipped.len());
                for page in &outcome.pages {
                    let size = std::fs::metadata(page).map(|m| m.len()).unwrap_or(0);
                    println!("  {:>9}  {}", size, page.display());
                }
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("crossfoot: {message}");
                ExitCode::FAILURE
            }
        },
        Command::Selectors { signatures } => {
            print_selectors(&signatures);
            ExitCode::SUCCESS
        }
        Command::Fetch {
            target: Target::Svzchf(opts),
        } => match run_svzchf(opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("crossfoot: {message}");
                ExitCode::FAILURE
            }
        },
        Command::Run {
            target: RunTarget::Mtbill(opts),
        } => match check_mtbill(opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("crossfoot: {message}");
                ExitCode::FAILURE
            }
        },
        Command::Run {
            target: RunTarget::Svzchf(opts),
        } => match recompute_svzchf(opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("crossfoot: {message}");
                ExitCode::FAILURE
            }
        },
    }
}

const BUILT_IN_SIGNATURES: [&str; 8] = [
    "asset()",
    "savings()",
    "totalSupply()",
    "totalAssets()",
    "convertToAssets(uint256)",
    "currentRatePPM()",
    "currentTicks()",
    "INTEREST_DELAY()",
];

fn print_selectors(signatures: &[String]) {
    let owned: Vec<String> = if signatures.is_empty() {
        BUILT_IN_SIGNATURES.iter().map(|s| s.to_string()).collect()
    } else {
        signatures.to_vec()
    };
    for signature in owned {
        let hash = abi::keccak256(signature.as_bytes());
        println!(
            "0x{}  0x{}  {signature}",
            abi::hex_encode(&hash),
            abi::hex_encode(&hash[0..4])
        );
    }
}

fn run_svzchf(opts: FetchOpts) -> Result<(), String> {
    let verify_root = opts.verify_root.canonicalize().map_err(|err| {
        format!(
            "--verify-root {} is not readable: {err}",
            opts.verify_root.display()
        )
    })?;

    let endpoints = if opts.endpoints.is_empty() {
        vec![
            DEFAULT_ARCHIVE_ENDPOINT.to_string(),
            DEFAULT_LATEST_ENDPOINT.to_string(),
        ]
    } else {
        opts.endpoints.clone()
    };

    let log_endpoints = if opts.log_endpoints.is_empty() {
        vec![DEFAULT_LOG_HISTORY_ENDPOINT.to_string()]
    } else {
        opts.log_endpoints.clone()
    };

    let cache = Cache::new(verify_root.join("cache"));
    let mut client = Client::new(
        endpoints,
        log_endpoints,
        cache,
        svzchf::EXPECTED_CHAIN_ID,
        opts.offline,
        opts.rpc_delay_ms,
    );

    let args = svzchf::FetchArgs {
        block: opts.block,
        baseline_block: opts.baseline_block,
        log_source: if opts.skip_logs {
            LogSource::None
        } else {
            match opts.log_source {
                LogSourceArg::Blockscout => LogSource::Blockscout,
                LogSourceArg::Rpc => LogSource::Rpc,
                LogSourceArg::None => LogSource::None,
            }
        },
        full_log_history: opts.full_log_history,
        max_log_chunks: opts.max_log_chunks,
        log_chunk: opts.log_chunk,
    };

    let outcome = svzchf::run(&mut client, &args, &verify_root)?;

    println!("bundle          {}", outcome.bundle_dir.display());
    println!("raw responses   {}", outcome.entry_count);
    println!("cache hits      {}", outcome.cache_hits);
    println!("network calls   {}", outcome.network_calls);
    println!("findings        {}", outcome.findings.len());
    for finding in &outcome.findings {
        println!("  [{}] {}: {}", finding.kind, finding.label, finding.detail);
    }
    Ok(())
}

fn recompute_svzchf(opts: RunOpts) -> Result<(), String> {
    let verify_root = opts.verify_root.canonicalize().map_err(|err| {
        format!(
            "--verify-root {} is not readable: {err}",
            opts.verify_root.display()
        )
    })?;

    let endpoints = if opts.endpoints.is_empty() {
        vec![
            DEFAULT_ARCHIVE_ENDPOINT.to_string(),
            DEFAULT_LATEST_ENDPOINT.to_string(),
        ]
    } else {
        opts.endpoints.clone()
    };
    let log_endpoints = if opts.log_endpoints.is_empty() {
        vec![DEFAULT_LOG_HISTORY_ENDPOINT.to_string()]
    } else {
        opts.log_endpoints.clone()
    };

    let cache = Cache::new(verify_root.join("cache"));
    let mut client = Client::new(
        endpoints,
        log_endpoints,
        cache,
        svzchf::EXPECTED_CHAIN_ID,
        opts.offline,
        opts.rpc_delay_ms,
    );

    let outcome = run_svzchf::run(
        &mut client,
        &run_svzchf::RunArgs {
            baseline_block: opts.baseline_block,
            block: opts.block,
        },
        &verify_root,
    )?;

    println!("verdict         {}", outcome.verdict.as_str());
    println!("result          {}", outcome.result_path.display());
    println!("bundle          {}", outcome.bundle_dir.display());
    println!("cache hits      {}", outcome.cache_hits);
    println!("network calls   {}", outcome.network_calls);
    Ok(())
}

fn check_mtbill(opts: RunOpts) -> Result<(), String> {
    let verify_root = opts.verify_root.canonicalize().map_err(|err| {
        format!(
            "--verify-root {} is not readable: {err}",
            opts.verify_root.display()
        )
    })?;

    let endpoints = if opts.endpoints.is_empty() {
        vec![
            DEFAULT_ARCHIVE_ENDPOINT.to_string(),
            DEFAULT_LATEST_ENDPOINT.to_string(),
        ]
    } else {
        opts.endpoints.clone()
    };
    let log_endpoints = if opts.log_endpoints.is_empty() {
        vec![DEFAULT_LOG_HISTORY_ENDPOINT.to_string()]
    } else {
        opts.log_endpoints.clone()
    };

    let cache = Cache::new(verify_root.join("cache"));
    let mut client = Client::new(
        endpoints,
        log_endpoints,
        cache,
        svzchf::EXPECTED_CHAIN_ID,
        opts.offline,
        opts.rpc_delay_ms,
    );

    let outcome = run_mtbill::run(
        &mut client,
        &run_mtbill::RunArgs {
            baseline_block: opts.baseline_block,
            block: opts.block,
        },
        &verify_root,
    )?;

    println!("nav_recomputation  INPUT_GAP (underlying portfolio not observable)");
    println!("consistency        {}", outcome.overall);
    for check in &outcome.checks {
        println!(
            "  {} {:<38} {:<20} {}",
            check.id,
            check.name,
            check.verdict.as_str(),
            check.summary
        );
    }
    println!("result             {}", outcome.result_path.display());
    println!("bundle             {}", outcome.bundle_dir.display());
    println!("cache hits         {}", outcome.cache_hits);
    println!("network calls      {}", outcome.network_calls);
    Ok(())
}
