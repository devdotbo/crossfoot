//! crossfoot: Crossfoot evidence collection CLI.
//!
//! Read-only by construction. The RPC layer issues eth_chainId, eth_call,
//! eth_getCode, eth_getBlockByNumber, eth_getLogs, eth_getTransactionByHash
//! and web3_clientVersion and nothing else; there is no signing key, no
//! eth_sendTransaction and no eth_sendRawTransaction anywhere in this binary.

mod abi;
mod bundle;
mod cache;
mod consume;
#[cfg(test)]
mod family_fixture_tests;
#[cfg(test)]
mod fixtures;
#[cfg(test)]
mod live_tests;
mod midas;
#[cfg(test)]
mod midas_fixture_tests;
mod model;
mod mtbill;
mod pack;
mod render;
mod rpc;
mod run_midas;
mod run_mtbill;
mod run_sky;
mod run_susde;
mod run_svzchf;
mod sky;
mod source;
mod summary;
mod susde;
mod svzchf;
mod util;
mod verify;

use std::path::{Path, PathBuf};
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
    /// Decide ALLOW or REVIEW per feed from the subgraph joined with the
    /// Crossfoot feed table, and write decisions/<stamp>/.
    Consume(consume::ConsumeOpts),
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
    /// Pack an evidence bundle into a deterministic archive.
    Bundle {
        #[command(subcommand)]
        action: BundleAction,
    },
    /// Re-hash every file of an evidence bundle and recompute its result
    /// from the bundle's own raw responses, without the network.
    Verify {
        /// The bundle directory, or a .tar.gz written by `bundle pack`.
        bundle: PathBuf,

        /// Exit 5 when the bundle was produced by different code. Without
        /// it a code difference is printed as a warning.
        #[arg(long)]
        require_same_code: bool,

        /// Re-read this many JSON-RPC entries ("all" for every one) from
        /// the endpoints at their pinned blocks and compare the results
        /// with the bundle's; a difference is exit 6. Only with this flag
        /// does verify touch the network.
        #[arg(long)]
        refetch: Option<verify::Sample>,

        /// JSON-RPC endpoints for --refetch, tried in order. The defaults
        /// when not given.
        #[arg(long = "endpoint", requires = "refetch")]
        endpoints: Vec<String>,
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
enum BundleAction {
    /// Write <bundle>.tar.gz: entries sorted, mtime 0, uid and gid 0, a
    /// fixed gzip header, so two packs of one bundle are byte-identical.
    /// Prints the archive sha256 and the bundle root hash.
    Pack {
        /// The bundle directory.
        bundle: PathBuf,

        /// Where to write the archive. Default: next to the bundle, named
        /// after it.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

#[derive(Subcommand)]
enum RunTarget {
    /// Frankencoin savings vault svZCHF.
    Svzchf(RunOpts),
    /// Midas mTBILL consistency bundle.
    Mtbill(RunOpts),
    /// Ethena sUSDe: exact recomputation from five state reads, reward
    /// posts attributed to their path.
    Susde(RunOpts),
    /// Sky sUSDS, sDAI and stUSDS: rpow to the wei, every rate change
    /// attributed to the bounded setter or the spell path.
    Sky(RunOpts),
    /// Midas customFeed family: posting-path replay of every feed in the list.
    Midas(MidasOpts),
    /// Any posted-feed family from its config file: `--config
    /// config/<family>-mainnet.json`. `run midas` is this with the Midas
    /// config as the default.
    Family(MidasOpts),
}

#[derive(Args)]
struct MidasOpts {
    /// Block every state read is pinned to; logs are swept over [0, block].
    #[arg(long)]
    block: u64,

    /// The family config: feed list, mechanism and explorer.
    #[arg(
        long = "config",
        visible_alias = "feeds",
        default_value = "config/midas-mainnet.json"
    )]
    feeds: PathBuf,

    /// Restrict the run to one product (`mRE7`) or one entry (`mFONE.mFONEUnloop.customFeed`).
    #[arg(long)]
    feed: Option<String>,

    /// A feed whose last post is older than this at the pinned block is stale.
    #[arg(long, default_value_t = 30)]
    stale_after_days: u64,

    /// The recent subset of the family summary counts bypasses within this
    /// many days before the pinned block.
    #[arg(long, default_value_t = 183)]
    recent_days: u64,

    /// JSON-RPC endpoint that serves trace_transaction or
    /// debug_traceTransaction, consulted only for rounds whose outer call is
    /// neither the feed nor a Safe.
    #[arg(long)]
    trace_endpoint: Option<String>,

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

    /// Serve every read from this evidence bundle's raw responses instead
    /// of the cache or the network, and fail on a read it does not hold.
    /// Reproduces a run from a checked-in fixture without a cache.
    #[arg(
        long,
        conflicts_with_all = ["offline", "endpoints", "log_endpoints", "trace_endpoint"]
    )]
    from_bundle: Option<PathBuf>,

    /// Wait this many milliseconds before each network call.
    #[arg(long, default_value_t = 0)]
    rpc_delay_ms: u64,
}

#[derive(Args)]
struct RunOpts {
    /// Start of the pinned window. The model is seeded from chain state here.
    #[arg(long, conflicts_with = "window", requires = "block")]
    baseline_block: Option<u64>,

    /// End of the pinned window. The model is compared against chain state here.
    #[arg(long, conflicts_with = "window", requires = "baseline_block")]
    block: Option<u64>,

    /// A named window preset instead of explicit blocks. "demo" for svzchf
    /// is the pinned pair 24570000 to 25853000.
    #[arg(long, conflicts_with_all = ["baseline_block", "block"])]
    window: Option<String>,

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

    /// Serve every read from this evidence bundle's raw responses instead
    /// of the cache or the network, and fail on a read it does not hold.
    /// Reproduces a run from a checked-in fixture without a cache.
    #[arg(long, conflicts_with_all = ["offline", "endpoints", "log_endpoints"])]
    from_bundle: Option<PathBuf>,

    /// Wait this many milliseconds before each network call.
    #[arg(long, default_value_t = 0)]
    rpc_delay_ms: u64,
}

/// The read source a run command uses: a bundle when `--from-bundle` was
/// given, else the network client over the workspace cache.
fn read_source(
    opts: &RunOpts,
    verify_root: &std::path::Path,
) -> Result<Box<dyn rpc::ReadSource>, String> {
    if let Some(bundle) = &opts.from_bundle {
        return Ok(Box::new(source::BundleSource::open(bundle)?));
    }
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
    Ok(Box::new(Client::new(
        endpoints,
        log_endpoints,
        Cache::new(verify_root.join("cache")),
        svzchf::EXPECTED_CHAIN_ID,
        opts.offline,
        opts.rpc_delay_ms,
    )))
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

/// A resolved run window: the two pinned blocks and the preset name when one
/// was used, so meta.json can record it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Window {
    baseline_block: u64,
    block: u64,
    name: Option<String>,
}

/// The pinned presets, per target. The svzchf demo window is the pair the
/// live tests assert (spec 01 R1).
fn window_preset(target: &str, name: &str) -> Option<(u64, u64)> {
    match (target, name) {
        ("svzchf", "demo") => Some(run_svzchf::DEMO_WINDOW),
        ("susde", "demo") => Some(run_susde::DEMO_WINDOW),
        ("sky", "demo") => Some(run_sky::DEMO_WINDOW),
        _ => None,
    }
}

fn resolve_window(target: &str, opts: &RunOpts) -> Result<Window, String> {
    match (&opts.window, opts.baseline_block, opts.block) {
        (Some(name), _, _) => {
            let (baseline_block, block) = window_preset(target, name)
                .ok_or_else(|| format!("there is no window preset named {name} for {target}"))?;
            Ok(Window {
                baseline_block,
                block,
                name: Some(name.clone()),
            })
        }
        (None, Some(baseline_block), Some(block)) => Ok(Window {
            baseline_block,
            block,
            name: None,
        }),
        _ => Err("give either --window <name> or both --baseline-block and --block".to_string()),
    }
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Command::Consume(opts) => match consume::run(&opts) {
            Ok(outcome) => {
                consume::print_outcome(&outcome);
                ExitCode::SUCCESS
            }
            Err(message) => {
                eprintln!("crossfoot: {message}");
                ExitCode::FAILURE
            }
        },
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
        Command::Bundle {
            action: BundleAction::Pack { bundle, out },
        } => {
            let out = out.unwrap_or_else(|| {
                let name = bundle
                    .file_name()
                    .map(|n| n.to_string_lossy().to_string())
                    .unwrap_or_else(|| "bundle".to_string());
                bundle
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_default()
                    .join(format!("{name}.tar.gz"))
            });
            match pack::pack(&bundle, &out) {
                Ok(packed) => {
                    println!("archive         {}", packed.archive.display());
                    println!("archive sha256  {}", packed.archive_sha256);
                    println!(
                        "root hash       {}",
                        packed.root_hash.as_deref().unwrap_or("not sealed")
                    );
                    println!("files           {}", packed.files);
                    ExitCode::SUCCESS
                }
                Err(message) => {
                    eprintln!("crossfoot: {message}");
                    ExitCode::FAILURE
                }
            }
        }
        Command::Verify {
            bundle,
            require_same_code,
            refetch,
            endpoints,
        } => {
            let report = verify::verify(
                &bundle,
                &verify::Options {
                    require_same_code,
                    refetch,
                    endpoints,
                },
            );
            for line in &report.lines {
                println!("{line}");
            }
            ExitCode::from(report.exit_code)
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
            target: RunTarget::Midas(opts) | RunTarget::Family(opts),
        } => match replay_midas(opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("crossfoot: {message}");
                ExitCode::FAILURE
            }
        },
        Command::Run {
            target: RunTarget::Sky(opts),
        } => match recompute_sky(opts) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("crossfoot: {message}");
                ExitCode::FAILURE
            }
        },
        Command::Run {
            target: RunTarget::Susde(opts),
        } => match recompute_susde(opts) {
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
    println!("root hash       {}", outcome.root_hash);
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
    let window = resolve_window("svzchf", &opts)?;
    let mut client = read_source(&opts, &verify_root)?;

    let outcome = run_svzchf::run(
        client.as_mut(),
        &run_svzchf::RunArgs {
            baseline_block: window.baseline_block,
            block: window.block,
            window_name: window.name.clone(),
        },
        &verify_root,
    )?;

    println!("verdict         {}", outcome.verdict.as_str());
    println!("summary         {}", outcome.summary.headline);
    println!("result          {}", outcome.result_path.display());
    println!("bundle          {}", outcome.bundle_dir.display());
    println!("root hash       {}", outcome.root_hash);
    println!("cache hits      {}", outcome.cache_hits);
    println!("network calls   {}", outcome.network_calls);
    Ok(())
}

fn recompute_susde(opts: RunOpts) -> Result<(), String> {
    let verify_root = opts.verify_root.canonicalize().map_err(|err| {
        format!(
            "--verify-root {} is not readable: {err}",
            opts.verify_root.display()
        )
    })?;
    let window = resolve_window("susde", &opts)?;
    let mut client = read_source(&opts, &verify_root)?;
    let outcome = run_susde::run(
        client.as_mut(),
        &run_susde::RunArgs {
            baseline_block: window.baseline_block,
            block: window.block,
            window_name: window.name.clone(),
        },
        &verify_root,
    )?;
    println!("verdict         {}", outcome.verdict.as_str());
    println!("summary         {}", outcome.summary.headline);
    println!("reward posts    {} in the window", outcome.posts_in_window);
    println!("result          {}", outcome.result_path.display());
    println!("bundle          {}", outcome.bundle_dir.display());
    println!("root hash       {}", outcome.root_hash);
    println!("cache hits      {}", outcome.cache_hits);
    println!("network calls   {}", outcome.network_calls);
    Ok(())
}

fn recompute_sky(opts: RunOpts) -> Result<(), String> {
    let verify_root = opts.verify_root.canonicalize().map_err(|err| {
        format!(
            "--verify-root {} is not readable: {err}",
            opts.verify_root.display()
        )
    })?;
    let window = resolve_window("sky", &opts)?;
    let mut client = read_source(&opts, &verify_root)?;
    let outcome = run_sky::run(
        client.as_mut(),
        &run_sky::RunArgs {
            baseline_block: window.baseline_block,
            block: window.block,
            window_name: window.name.clone(),
        },
        &verify_root,
    )?;
    println!("verdict         {}", outcome.verdict.as_str());
    println!("summary         {}", outcome.summary.headline);
    println!("rate changes    {} in the window", outcome.rate_changes);
    println!("result          {}", outcome.result_path.display());
    println!("bundle          {}", outcome.bundle_dir.display());
    println!("root hash       {}", outcome.root_hash);
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
    let window = resolve_window("mtbill", &opts)?;
    let mut client = read_source(&opts, &verify_root)?;

    let outcome = run_mtbill::run(
        client.as_mut(),
        &run_mtbill::RunArgs {
            baseline_block: window.baseline_block,
            block: window.block,
            window_name: window.name.clone(),
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
    println!("summary            {}", outcome.summary.headline);
    println!("result             {}", outcome.result_path.display());
    println!("bundle             {}", outcome.bundle_dir.display());
    println!("root hash          {}", outcome.root_hash);
    println!("cache hits         {}", outcome.cache_hits);
    println!("network calls      {}", outcome.network_calls);
    Ok(())
}

fn replay_midas(opts: MidasOpts) -> Result<(), String> {
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

    let list = midas::load_feed_list(&opts.feeds)?;
    let feeds = midas::select_feeds(&list, opts.feed.as_deref())?;

    let mut client: Box<dyn rpc::ReadSource> = match &opts.from_bundle {
        Some(bundle) => Box::new(source::BundleSource::open(bundle)?),
        None => Box::new(Client::new(
            endpoints,
            log_endpoints,
            Cache::new(verify_root.join("cache")),
            list.chain_id,
            opts.offline,
            opts.rpc_delay_ms,
        )),
    };
    // Traces come from the same bundle on a replay, else from the trace
    // endpoint when one was given.
    let mut trace_bundle = match &opts.from_bundle {
        Some(bundle) => Some(source::BundleSource::open(bundle)?),
        None => None,
    };
    let mut trace_net = match (&opts.from_bundle, &opts.trace_endpoint) {
        (None, Some(url)) => Some(Client::new(
            vec![url.clone()],
            Vec::new(),
            Cache::new(verify_root.join("cache")),
            list.chain_id,
            opts.offline,
            opts.rpc_delay_ms,
        )),
        _ => None,
    };

    let outcome = run_midas::run(
        client.as_mut(),
        run_midas::RunArgs {
            block: opts.block,
            target: list.target(),
            family: list.family.clone(),
            explorer: list.explorer.clone(),
            mechanism: list.mechanism.clone(),
            feeds,
            feed_list_source: opts.feeds.display().to_string(),
            stale_after_days: opts.stale_after_days,
            recent_days: opts.recent_days,
            trace: trace_bundle
                .as_mut()
                .map(|c| c as &mut dyn rpc::ReadSource)
                .or_else(|| trace_net.as_mut().map(|c| c as &mut dyn rpc::ReadSource)),
        },
        &verify_root,
    )?;

    println!(
        "nav_recomputation  INPUT_GAP (no NAV is recomputed; findings are about the posting path)"
    );
    println!("survey             {}", outcome.survey_line);
    println!("verdict            {}", outcome.verdict);
    for row in &outcome.rows {
        println!(
            "  {:<36} {:<22} {:>3}  {:<22} {:<12} {}",
            row.name, row.posts, row.bypasses, row.posting_path, row.liveness, row.verdict
        );
    }
    println!("result             {}", outcome.result_path.display());
    println!("bundle             {}", outcome.bundle_dir.display());
    println!("root hash          {}", outcome.root_hash);
    println!("cache hits         {}", outcome.cache_hits);
    println!("network calls      {}", outcome.network_calls);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run_opts(args: &[&str]) -> Result<RunOpts, clap::Error> {
        let mut argv = vec!["crossfoot", "run", "svzchf"];
        argv.extend_from_slice(args);
        let cli = Cli::try_parse_from(argv)?;
        match cli.command {
            Command::Run {
                target: RunTarget::Svzchf(opts),
            } => Ok(opts),
            _ => panic!("the arguments parse as run svzchf"),
        }
    }

    /// Spec 01 R1: the preset is exactly the pinned pair the live tests use.
    #[test]
    fn window_preset_demo_expands_to_the_pinned_blocks() {
        let opts = run_opts(&["--window", "demo"]).unwrap();
        let window = resolve_window("svzchf", &opts).unwrap();
        assert_eq!(
            window,
            Window {
                baseline_block: 24_570_000,
                block: 25_853_000,
                name: Some("demo".to_string()),
            }
        );

        let explicit = run_opts(&["--baseline-block", "24570000", "--block", "25853000"]).unwrap();
        let window = resolve_window("svzchf", &explicit).unwrap();
        assert_eq!(window.baseline_block, 24_570_000);
        assert_eq!(window.block, 25_853_000);
        assert_eq!(window.name, None);

        let susde = resolve_window("susde", &opts).unwrap();
        assert_eq!(
            (susde.baseline_block, susde.block),
            (25_800_000, 25_885_407)
        );
        // No preset of that name for the other target.
        let err = resolve_window("mtbill", &opts).unwrap_err();
        assert!(
            err.contains("no window preset named demo for mtbill"),
            "{err}"
        );
    }

    /// `--from-bundle` replaces the cache and the network: it conflicts
    /// with the flags that describe those, and a run from the checked-in
    /// fixture writes the fixture's result.json byte for byte.
    #[test]
    fn run_from_bundle_reproduces_the_fixture_result() {
        assert!(run_opts(&["--window", "demo", "--from-bundle", "x", "--offline"]).is_err());
        assert!(run_opts(&["--window", "demo", "--from-bundle", "x", "--endpoint", "u"]).is_err());
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/svzchf-demo-24570000-25853000");
        let opts = run_opts(&[
            "--window",
            "demo",
            "--from-bundle",
            fixture.to_str().unwrap(),
        ])
        .unwrap();
        let root =
            std::env::temp_dir().join(format!("crossfoot-from-bundle-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let window = resolve_window("svzchf", &opts).unwrap();
        let mut source = read_source(&opts, &root).unwrap();
        let outcome = run_svzchf::run(
            source.as_mut(),
            &run_svzchf::RunArgs {
                baseline_block: window.baseline_block,
                block: window.block,
                window_name: window.name,
            },
            &root,
        )
        .unwrap();
        assert_eq!(outcome.verdict, model::verdict::Verdict::ModelMatch);
        assert_eq!(outcome.network_calls, 0);
        assert_eq!(
            std::fs::read(&outcome.result_path).unwrap(),
            std::fs::read(fixture.join("result.json")).unwrap()
        );
    }

    #[test]
    fn window_and_explicit_blocks_are_mutually_exclusive() {
        assert!(run_opts(&["--window", "demo", "--block", "25853000"]).is_err());
        assert!(run_opts(&["--window", "demo", "--baseline-block", "24570000"]).is_err());
        // One explicit block without the other is not a window either.
        assert!(run_opts(&["--block", "25853000"]).is_err());
        assert!(run_opts(&["--baseline-block", "24570000"]).is_err());
        // Nothing at all is a resolution error, not a parse error.
        let none = run_opts(&[]).unwrap();
        assert!(resolve_window("svzchf", &none).is_err());
    }
}
