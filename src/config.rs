use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

/// Built-in profiles. "linux" can be overridden via DISTQ_REPOS env var.
pub fn builtin_profiles() -> HashMap<&'static str, Vec<&'static str>> {
    let mut m = HashMap::new();
    m.insert(
        "linux",
        vec![
            "arch",
            "debian_unstable",
            "fedora_rawhide",
            "ubuntu_24_04",
            "alpine_edge",
            "nixos",
            "gentoo",
            "void_x86_64",
        ],
    );
    m.insert(
        "bsd",
        vec!["freebsd", "openbsd", "pkgsrc_current", "homebrew"],
    );
    m.insert(
        "all",
        vec![
            "arch",
            "debian_unstable",
            "fedora_rawhide",
            "ubuntu_24_04",
            "alpine_edge",
            "nixos",
            "gentoo",
            "void_x86_64",
            "freebsd",
            "openbsd",
            "pkgsrc_current",
        ],
    );
    m
}

#[derive(Debug, Deserialize, Serialize, Default)]
pub struct Config {
    /// User-defined profiles: profile name → list of repo identifiers.
    #[serde(default)]
    pub profiles: HashMap<String, Vec<String>>,

    /// Default repos used when neither --repos, --profile, --repo, nor DISTQ_REPOS is set.
    /// If absent, falls back to autodetect.
    #[serde(default)]
    pub default_repos: Vec<String>,
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = config_path();
        if !path.exists() {
            return Ok(Self::default());
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        toml::from_str(&text)
            .with_context(|| format!("parsing {}", path.display()))
    }

    /// Resolve a named profile to a list of repo strings.
    /// Priority: user config profiles > built-in profiles.
    pub fn resolve_profile<'a>(&'a self, name: &str) -> Option<Vec<String>> {
        if let Some(repos) = self.profiles.get(name) {
            return Some(repos.clone());
        }
        let builtins = builtin_profiles();
        builtins
            .get(name)
            .map(|repos| repos.iter().map(|r| r.to_string()).collect())
    }
}

pub fn config_path() -> PathBuf {
    dirs_next::config_dir()
        .unwrap_or_else(|| PathBuf::from("~/.config"))
        .join("distq")
        .join("config.toml")
}

/// Parse a comma-separated list of repo names, normalizing each one.
pub fn parse_repos(s: &str) -> Vec<String> {
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|r| crate::normalize_repo(r.to_string()))
        .collect()
}
