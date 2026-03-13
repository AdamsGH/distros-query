use anyhow::{bail, Result};
use std::collections::HashSet;
use std::process::Command;
use std::sync::OnceLock;
use tokio::task::spawn_blocking;

use crate::sources::{PackageInfo, PackageSource, PackageStatus};

pub struct DockerSource;

// Repo identifiers that DockerSource can handle.
// Each entry maps a distq repo id → docker image tag suffix + parser variant.
struct Mapping {
    /// Repo prefix accepted by `supports()`.
    repo_prefix: &'static str,
    /// Image tag: `distq/<tag>`.
    image: &'static str,
    parser: Parser,
}

#[derive(Clone, Copy)]
enum Parser {
    /// One package name per line (apk).
    OneName,
    /// "name - description" (apt-cache).
    AptCache,
    /// "repo/name version\n    description" (pacman -Ss).
    Pacman,
    /// "name.arch : description" (microdnf/dnf).
    Dnf,
    /// "| S | name | summary |" (zypper).
    Zypper,
    /// "[-] name-ver_rev description" (xbps-query -Rs).
    Xbps,
}

static MAPPINGS: &[Mapping] = &[
    Mapping { repo_prefix: "alpine",   image: "alpine",   parser: Parser::OneName  },
    Mapping { repo_prefix: "arch",     image: "arch",     parser: Parser::Pacman   },
    Mapping { repo_prefix: "debian",   image: "debian",   parser: Parser::AptCache },
    Mapping { repo_prefix: "ubuntu",   image: "ubuntu",   parser: Parser::AptCache },
    Mapping { repo_prefix: "fedora",   image: "fedora",   parser: Parser::Dnf      },
    Mapping { repo_prefix: "opensuse", image: "opensuse", parser: Parser::Zypper   },
    Mapping { repo_prefix: "void",     image: "void",     parser: Parser::Xbps     },
];

fn mapping_for(repo: &str) -> Option<&'static Mapping> {
    MAPPINGS.iter().find(|m| repo == m.repo_prefix || repo.starts_with(m.repo_prefix))
}

/// Return the set of distq/* image suffixes present locally, queried once and cached.
fn available_images() -> &'static HashSet<String> {
    static CACHE: OnceLock<HashSet<String>> = OnceLock::new();
    CACHE.get_or_init(|| {
        let out = Command::new("docker")
            .args(["images", "--filter", "reference=distq/*", "--format", "{{.Repository}}"])
            .output();
        match out {
            Err(_) => HashSet::new(),
            Ok(o) => String::from_utf8_lossy(&o.stdout)
                .lines()
                // "distq/arch" → "arch"
                .filter_map(|l| l.trim().strip_prefix("distq/").map(str::to_string))
                .collect(),
        }
    })
}

#[async_trait::async_trait]
impl PackageSource for DockerSource {
    fn name(&self) -> &'static str { "docker" }

    fn supports(&self, repo: &str) -> bool {
        mapping_for(repo)
            .map(|m| available_images().contains(m.image))
            .unwrap_or(false)
    }

    async fn search(
        &self,
        pkg: &str,
        repo: &str,
        _client: &reqwest::Client,
    ) -> Result<Vec<PackageInfo>> {
        let mapping = match mapping_for(repo) {
            Some(m) => m,
            None => bail!("docker: no mapping for repo '{repo}'"),
        };

        let image = format!("distq/{}", mapping.image);
        let pkg = pkg.to_string();
        let repo = repo.to_string();
        let parser = mapping.parser;

        // docker run is a blocking subprocess — run it off the async executor.
        let output = spawn_blocking(move || {
            Command::new("docker")
                .args(["run", "--rm", "--network=none", &image, &pkg])
                .output()
                .map(|o| (String::from_utf8_lossy(&o.stdout).into_owned(), pkg, repo))
        })
        .await??;

        let (stdout, pkg, repo) = output;
        Ok(parse(&pkg, &repo, parser, &stdout))
    }
}

fn parse(pkg: &str, repo: &str, parser: Parser, output: &str) -> Vec<PackageInfo> {
    let all = match parser {
        Parser::OneName  => parse_one_name(pkg, repo, output),
        Parser::AptCache => parse_apt_cache(pkg, repo, output),
        Parser::Pacman   => parse_pacman(pkg, repo, output),
        Parser::Dnf      => parse_dnf(pkg, repo, output),
        Parser::Zypper   => parse_zypper(pkg, repo, output),
        Parser::Xbps     => parse_xbps(pkg, repo, output),
    };
    filter_results(pkg, all)
}

/// Package managers search by substring — prefer exact match, fall back to all results.
/// This keeps behaviour consistent with native API sources (arch, alpine, etc.)
/// which return only the exact package.
fn filter_results(pkg: &str, results: Vec<PackageInfo>) -> Vec<PackageInfo> {
    let exact: Vec<PackageInfo> = results
        .iter()
        .filter(|r| r.name == pkg)
        .cloned()
        .collect();
    if exact.is_empty() { results } else { exact }
}

fn make(pkg: &str, repo: &str, name: String, version: String) -> PackageInfo {
    PackageInfo {
        query_name:  pkg.to_string(),
        name,
        repo:        repo.to_string(),
        version,
        status:      PackageStatus::Unknown,
        latest:      "-".to_string(),
        maintainers: vec![],
        source:      "docker",
    }
}

// apk search -q → one name per line, no version
fn parse_one_name(pkg: &str, repo: &str, output: &str) -> Vec<PackageInfo> {
    output
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .map(|name| make(pkg, repo, name.to_string(), "-".to_string()))
        .collect()
}

// apt-cache search --names-only → "name - short description"
fn parse_apt_cache(pkg: &str, repo: &str, output: &str) -> Vec<PackageInfo> {
    output
        .lines()
        .filter_map(|l| {
            let (name, _desc) = l.split_once(" - ")?;
            let name = name.trim().to_string();
            if name.is_empty() { return None; }
            Some(make(pkg, repo, name, "-".to_string()))
        })
        .collect()
}

// pacman -Ss → pairs of lines:
//   "repo/name version (groups)\n    description"
fn parse_pacman(pkg: &str, repo: &str, output: &str) -> Vec<PackageInfo> {
    let mut results = Vec::new();
    let mut lines = output.lines().peekable();
    while let Some(line) = lines.next() {
        let line = line.trim();
        if line.is_empty() || line.starts_with(' ') { continue; }
        // "core/curl 8.9.0-1 [installed]"
        if let Some(slash) = line.find('/') {
            let rest = &line[slash + 1..];
            // rest = "name version ..."
            let mut parts = rest.split_whitespace();
            if let Some(name) = parts.next() {
                let version = parts.next().unwrap_or("-").to_string();
                results.push(make(pkg, repo, name.to_string(), version));
            }
        }
        // skip description line
        let _ = lines.next();
    }
    results
}

// dnf search output (Fedora 40+):
//   Header lines: "Updating and loading repositories:", "Matched fields: ...", separator lines
//   Data lines:   "name.arch\tSummary"  (tab-separated, no version)
fn parse_dnf(pkg: &str, repo: &str, output: &str) -> Vec<PackageInfo> {
    output
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty()
                || l.starts_with('=')
                || l.starts_with("Updating")
                || l.starts_with("Repositories")
                || l.starts_with("Matched")
                || l.starts_with("Last")
            {
                return None;
            }
            // Data line: "curl.x86_64\tSummary" — split on first tab
            let name_arch = l.split('\t').next()?.trim();
            // Strip .arch suffix
            let name = name_arch.split('.').next()?.trim().to_string();
            if name.is_empty() { return None; }
            Some(make(pkg, repo, name, "-".to_string()))
        })
        .collect()
}

// zypper search output (openSUSE Leap 15.5):
//   "S  | Name   | Summary   | Type"
//   "---+--------+-----------+--------"
//   "   | curl   | A Tool... | package"
// No version in search output — zypper info <pkg> would be needed for that.
fn parse_zypper(pkg: &str, repo: &str, output: &str) -> Vec<PackageInfo> {
    output
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            // Skip separator lines and lines not containing '|'
            if !l.contains('|') || l.starts_with('-') { return None; }
            let fields: Vec<&str> = l.split('|')
                .map(|f| f.trim())
                .collect();
            // Need at least: S, Name, Summary — i.e. 3+ fields
            if fields.len() < 3 { return None; }
            // Header row
            if fields[1] == "Name" || fields[1] == "S" { return None; }
            let name = fields[1].to_string();
            if name.is_empty() { return None; }
            Some(make(pkg, repo, name, "-".to_string()))
        })
        .collect()
}

// xbps-query -Rs → "[-] name-version_rev description"
// The version is embedded in the pkgver field: split on last '-'
fn parse_xbps(pkg: &str, repo: &str, output: &str) -> Vec<PackageInfo> {
    output
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.is_empty() { return None; }
            // Strip leading "[-] " or "[*] "
            let rest = if l.len() > 4 && l.as_bytes()[0] == b'[' {
                l[4..].trim()
            } else {
                l
            };
            // "name-version_rev description"
            let pkgver = rest.split_whitespace().next()?;
            // pkgver = "curl-8.9.0_1" → split on last '-'
            let (name, version) = pkgver.rsplit_once('-')?;
            if name.is_empty() { return None; }
            Some(make(pkg, repo, name.to_string(), version.to_string()))
        })
        .collect()
}
