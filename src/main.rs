mod autodetect;
mod config;
mod format;
mod sources;

use anyhow::{Context, Result, bail};
use clap::Parser;
use config::{Config, parse_repos};
use format::{OutputFormat, TableLayout, print_results};
use sources::{PackageInfo, PackageSource, ordered_sources, source_for};
use sources::repology;
use std::io::{self, IsTerminal};
use std::sync::Arc;
use tokio::task::JoinSet;

const DEFAULT_JOBS: usize = 2;
const VERSION: &str = "0.1.0";

#[derive(Parser, Debug)]
#[command(
    name = "distq",
    about = "Query package information from Repology.org and native distro APIs",
    long_about = "Query package information across Linux distributions.\n\n\
        Source priority (per-repo, first match wins): arch → aur → fedora → repology\n\
        Override in ~/.config/distq/config.toml under [sources] priority = [...]\n\n\
        Repo selection priority (highest to lowest):\n  \
        --repos  >  DISTQ_REPOS env  >  --profile  >  config default_repos  >  autodetect",
    version = VERSION,
    disable_version_flag = true,
)]
struct Args {
    /// Package names to query (omit to list packages from your repo)
    #[arg(name = "PACKAGE")]
    packages: Vec<String>,

    // ── Repo / profile selection ───────────────────────────────────────────

    /// Single repository to query (autodetected if not set)
    #[arg(long, conflicts_with_all = ["REPO_LIST", "profile"])]
    repo: Option<String>,

    /// Comma-separated list of repositories: arch,debian,fedora,nixos
    #[arg(long = "repos", name = "REPO_LIST", conflicts_with_all = ["repo", "profile"])]
    multi_repos: Option<String>,

    /// Named profile of repositories: linux, bsd, all, or custom from config
    #[arg(long, conflicts_with_all = ["repo", "REPO_LIST"])]
    profile: Option<String>,

    // ── Display ────────────────────────────────────────────────────────────

    /// Show packages from all repositories (repology only)
    #[arg(long, conflicts_with_all = ["repo", "REPO_LIST", "profile"])]
    all: bool,

    /// Table layout: transposed (repo-per-row, default for multi-repo),
    /// flat (package-per-row, default for single-repo)
    #[arg(long)]
    layout: Option<TableLayout>,

    /// Output format: table (default), json
    #[arg(long, default_value = "table")]
    format: OutputFormat,

    // ── Repology API filters (used when repology backend is active) ────────

    /// Filter: packages present in this repo
    #[arg(long)]
    inrepo: Option<String>,

    /// Filter: packages absent from this repo
    #[arg(long)]
    notinrepo: Option<String>,

    /// Filter: project name substring
    #[arg(long)]
    search: Option<String>,

    /// Filter: maintainer email
    #[arg(long)]
    maintainer: Option<String>,

    /// Filter: package category
    #[arg(long)]
    category: Option<String>,

    /// Filter: at least this many repos carry the package
    #[arg(long)]
    min_repos: Option<String>,

    /// Filter: at least this many package families
    #[arg(long)]
    min_families: Option<String>,

    /// Filter: at least this many repos with the newest version
    #[arg(long)]
    min_repos_newest: Option<String>,

    /// Filter: at least this many families with the newest version
    #[arg(long)]
    min_families_newest: Option<String>,

    /// Filter: only newest packages
    #[arg(long)]
    newest: bool,

    /// Filter: only outdated packages
    #[arg(long)]
    outdated: bool,

    /// Filter: only problematic packages
    #[arg(long)]
    problematic: bool,

    // ── Listing / pagination (repology listing mode) ───────────────────────

    /// Start listing from this project name
    #[arg(long)]
    begin: Option<String>,

    /// End listing at this project name
    #[arg(long)]
    end: Option<String>,

    /// Fetch this many pages of 200 items each (listing mode only)
    #[arg(long, default_value = "1")]
    page: u32,

    /// Sort by binary package name instead of source name (repology only)
    #[arg(long)]
    sort_package: bool,

    // ── Misc ───────────────────────────────────────────────────────────────

    /// Maximum parallel HTTP requests
    #[arg(long, default_value_t = DEFAULT_JOBS)]
    jobs: usize,

    /// Print version and exit
    #[arg(long = "version", short = 'V')]
    print_version: bool,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    if args.print_version {
        println!("distq {VERSION}");
        return Ok(());
    }

    let cfg = Config::load().context("failed to load config")?;
    let color = supports_color();
    let jobs = args.jobs.max(1);

    let client = reqwest::Client::builder()
        .user_agent(concat!("distq/", env!("CARGO_PKG_VERSION")))
        .build()
        .context("failed to build HTTP client")?;

    // ── Resolve target repos ───────────────────────────────────────────────

    let repos: Vec<String> = if args.all {
        vec![]
    } else if let Some(ref list) = args.multi_repos {
        parse_repos(list)
    } else if let Some(ref env_val) = std::env::var("DISTQ_REPOS").ok().filter(|s| !s.is_empty()) {
        parse_repos(env_val)
    } else if let Some(ref profile_name) = args.profile {
        match cfg.resolve_profile(profile_name) {
            Some(r) => r.into_iter().map(normalize_repo).collect(),
            None => bail!(
                "distq: unknown profile '{profile_name}'\n\
                 Built-in profiles: linux, bsd, all\n\
                 Custom profiles are defined in {}",
                config::config_path().display()
            ),
        }
    } else if let Some(ref r) = args.repo {
        vec![normalize_repo(r.clone())]
    } else if !cfg.default_repos.is_empty() {
        cfg.default_repos.iter().map(|r| normalize_repo(r.clone())).collect()
    } else {
        match autodetect::detect() {
            Some(r) => vec![r],
            None => bail!(
                "distq: could not autodetect your repository\n\
                 Use --repo, --repos, --profile, or set DISTQ_REPOS\n\
                 See https://repology.org/repositories/statistics for repo names"
            ),
        }
    };

    let is_multi = repos.len() > 1;
    let layout = args.layout.unwrap_or(if is_multi {
        TableLayout::Transposed
    } else {
        TableLayout::Flat
    });

    // ── Build source registry ──────────────────────────────────────────────

    let sources = Arc::new(ordered_sources(&cfg));

    // ── Execute queries ────────────────────────────────────────────────────

    let results: Vec<PackageInfo> = if args.packages.is_empty() {
        // Listing mode — always via Repology (it's the only source with browse API)
        let repo = repos.into_iter().next().unwrap_or_default();
        let query = build_repology_filters(&args, &repo);
        if args.end.is_some() {
            repology::fetch_packages_list(&client, &repo, args.begin.as_deref(), args.end.as_deref(), query).await?
        } else {
            repology::fetch_pages(&client, &repo, args.begin.as_deref(), args.page, query).await?
        }
    } else if args.all {
        // --all goes straight to repology
        let query = build_repology_filters(&args, "");
        fetch_packages_parallel(&client, &args.packages, &[""], true, args.sort_package, query, jobs, &sources).await?
    } else {
        let query = build_repology_filters(&args, "");
        fetch_packages_parallel(&client, &args.packages, &repos.iter().map(|s| s.as_str()).collect::<Vec<_>>(), false, args.sort_package, query, jobs, &sources).await?
    };

    print_results(&results, args.format, layout, color);
    Ok(())
}

// ── Parallel fetch ────────────────────────────────────────────────────────────

async fn fetch_packages_parallel(
    client: &reqwest::Client,
    packages: &[String],
    repos: &[&str],
    all: bool,
    sort_package: bool,
    repology_filters: Vec<(String, String)>,
    jobs: usize,
    sources: &Arc<Vec<Box<dyn PackageSource>>>,
) -> Result<Vec<PackageInfo>> {
    let client = Arc::new(client.clone());
    let filters = Arc::new(repology_filters);
    let semaphore = Arc::new(tokio::sync::Semaphore::new(jobs));
    let mut set: JoinSet<Result<Vec<PackageInfo>>> = JoinSet::new();

    for pkg in packages {
        for &repo in repos {
            let client = Arc::clone(&client);
            let sources = Arc::clone(sources);
            let filters = Arc::clone(&filters);
            let pkg = pkg.clone();
            let repo = repo.to_string();
            let sem = Arc::clone(&semaphore);

            set.spawn(async move {
                let _permit = sem.acquire().await;

                // Route to the best available source for this repo.
                let source = source_for(&sources, &repo);

                match source {
                    Some(src) if src.name() != "repology" => {
                        src.search(&pkg, &repo, &client).await
                    }
                    _ => {
                        // Repology — pass through extra filters.
                        repology::fetch_package(&client, &pkg, &repo, all, sort_package, &filters).await
                    }
                }
            });
        }
    }

    let mut all_results = Vec::new();
    while let Some(res) = set.join_next().await {
        all_results.extend(res.context("task panicked")?.context("request failed")?);
    }
    all_results.sort_by(|a, b| a.name.cmp(&b.name).then(a.repo.cmp(&b.repo)));
    Ok(all_results)
}

// ── Query helpers ─────────────────────────────────────────────────────────────

/// Build Repology API filter params from CLI args.
/// inrepo is intentionally omitted here — each request adds it per-repo in repology.rs.
fn build_repology_filters(args: &Args, _repo: &str) -> Vec<(String, String)> {
    let mut p: Vec<(String, String)> = Vec::new();

    macro_rules! opt_str {
        ($field:expr, $key:expr) => {
            if let Some(v) = &$field { p.push(($key.into(), v.clone())); }
        };
    }
    macro_rules! opt_bool {
        ($field:expr, $key:expr) => {
            if $field { p.push(($key.into(), "1".into())); }
        };
    }

    opt_str!(args.inrepo,            "inrepo");
    opt_str!(args.notinrepo,         "notinrepo");
    opt_str!(args.search,            "search");
    opt_str!(args.maintainer,        "maintainer");
    opt_str!(args.category,          "category");
    opt_str!(args.min_repos,         "repos");
    opt_str!(args.min_families,      "families");
    opt_str!(args.min_repos_newest,  "repos_newest");
    opt_str!(args.min_families_newest, "families_newest");
    opt_bool!(args.newest,           "newest");
    opt_bool!(args.outdated,         "outdated");
    opt_bool!(args.problematic,      "problematic");

    p
}

// ── Misc ──────────────────────────────────────────────────────────────────────

pub fn normalize_repo(repo: String) -> String {
    match repo.as_str() {
        "alpine"             => "alpine_edge".into(),
        "debian"             => "debian_unstable".into(),
        "fedora"             => "fedora_rawhide".into(),
        "pkgsrc"             => "pkgsrc_current".into(),
        "opensuse" | "suse"  => "opensuse_tumbleweed".into(),
        "nix" | "nixos"      => "nix_unstable".into(),
        "void"               => "void_x86_64".into(),
        _                    => repo,
    }
}

fn supports_color() -> bool {
    if std::env::var_os("NO_COLOR").is_some() { return false; }
    io::stdout().is_terminal()
}
