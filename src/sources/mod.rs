pub mod arch;
pub mod aur;
pub mod fedora;
pub mod repology;

use anyhow::Result;
use crate::config::Config;

// ── Shared models ────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PackageInfo {
    /// Project name as originally queried — used for column grouping in transposed view.
    pub query_name: String,
    /// Package name in this repo (srcname / pkgname / etc).
    pub name: String,
    /// Repo/distro identifier, e.g. "arch", "fedora_rawhide".
    pub repo: String,
    pub version: String,
    pub status: PackageStatus,
    /// Latest known version across all repos (may be "-" if unknown).
    pub latest: String,
    pub maintainers: Vec<String>,
    /// Which source backend produced this result.
    #[allow(dead_code)]
    pub source: &'static str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackageStatus {
    Newest,
    Outdated,
    Devel,
    Legacy,
    Rolling,
    Unique,
    NoScheme,
    Incorrect,
    Untrusted,
    Ignored,
    Unknown,
}

impl PackageStatus {
    pub fn from_str(s: &str) -> Self {
        match s {
            "newest"    => Self::Newest,
            "outdated"  => Self::Outdated,
            "devel"     => Self::Devel,
            "legacy"    => Self::Legacy,
            "rolling"   => Self::Rolling,
            "unique"    => Self::Unique,
            "noscheme"  => Self::NoScheme,
            "incorrect" => Self::Incorrect,
            "untrusted" => Self::Untrusted,
            "ignored"   => Self::Ignored,
            _           => Self::Unknown,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Newest    => "newest",
            Self::Outdated  => "outdated",
            Self::Devel     => "devel",
            Self::Legacy    => "legacy",
            Self::Rolling   => "rolling",
            Self::Unique    => "unique",
            Self::NoScheme  => "noscheme",
            Self::Incorrect => "incorrect",
            Self::Untrusted => "untrusted",
            Self::Ignored   => "ignored",
            Self::Unknown   => "unknown",
        }
    }
}

// ── Trait ────────────────────────────────────────────────────────────────────

#[async_trait::async_trait]
pub trait PackageSource: Send + Sync {
    /// Short identifier shown in output and used in config priority list.
    fn name(&self) -> &'static str;

    /// Return true if this source knows how to handle the given repo identifier.
    /// Called before `search` to decide routing.
    fn supports(&self, repo: &str) -> bool;

    /// Look up a single package by exact project name.
    async fn search(
        &self,
        pkg: &str,
        repo: &str,
        client: &reqwest::Client,
    ) -> Result<Vec<PackageInfo>>;
}

// ── Registry ─────────────────────────────────────────────────────────────────

/// Default source priority when not overridden in config.
/// First source that `supports(repo)` and returns results wins.
pub const DEFAULT_PRIORITY: &[&str] = &["arch", "aur", "fedora", "repology"];

/// Build the ordered list of sources to try, respecting config priority.
pub fn ordered_sources(cfg: &Config) -> Vec<Box<dyn PackageSource>> {
    let priority: Vec<&str> = if cfg.source_priority.is_empty() {
        DEFAULT_PRIORITY.to_vec()
    } else {
        cfg.source_priority.iter().map(|s| s.as_str()).collect()
    };

    let mut result: Vec<Box<dyn PackageSource>> = Vec::new();
    for name in &priority {
        match *name {
            "arch"     => result.push(Box::new(arch::ArchSource)),
            "aur"      => result.push(Box::new(aur::AurSource)),
            "fedora"   => result.push(Box::new(fedora::FedoraSource)),
            "repology" => result.push(Box::new(repology::RepologySource::new())),
            other => eprintln!("distq: unknown source '{other}' in config, skipping"),
        }
    }

    // Always append repology as a last-resort fallback if not already present.
    if !priority.contains(&"repology") {
        result.push(Box::new(repology::RepologySource::new()));
    }

    result
}

/// Find the best source for a given repo, in priority order.
pub fn source_for<'a>(
    sources: &'a [Box<dyn PackageSource>],
    repo: &str,
) -> Option<&'a dyn PackageSource> {
    sources.iter().find(|s| s.supports(repo)).map(|s| s.as_ref())
}
