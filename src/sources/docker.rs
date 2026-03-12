use anyhow::{bail, Result};
use std::process::Command;

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

/// Check that image `distq/<tag>` exists locally without pulling.
fn image_exists(tag: &str) -> bool {
    Command::new("docker")
        .args(["image", "inspect", "--format", ".", &format!("distq/{tag}")])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[async_trait::async_trait]
impl PackageSource for DockerSource {
    fn name(&self) -> &'static str { "docker" }

    fn supports(&self, repo: &str) -> bool {
        mapping_for(repo)
            .map(|m| image_exists(m.image))
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

        let output = Command::new("docker")
            .args(["run", "--rm", "--network=none", &image, pkg])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let results = parse(pkg, repo, mapping.parser, &stdout);
        Ok(results)
    }
}

fn parse(pkg: &str, repo: &str, parser: Parser, output: &str) -> Vec<PackageInfo> {
    match parser {
        Parser::OneName  => parse_one_name(pkg, repo, output),
        Parser::AptCache => parse_apt_cache(pkg, repo, output),
        Parser::Pacman   => parse_pacman(pkg, repo, output),
        Parser::Dnf      => parse_dnf(pkg, repo, output),
        Parser::Zypper   => parse_zypper(pkg, repo, output),
        Parser::Xbps     => parse_xbps(pkg, repo, output),
    }
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

// microdnf/dnf search → "name.arch : description"
// May include header lines like "============" — skip those.
fn parse_dnf(pkg: &str, repo: &str, output: &str) -> Vec<PackageInfo> {
    output
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.starts_with('=') || l.is_empty() { return None; }
            // "curl.x86_64 : ..." or "curl : ..."
            let name_arch = l.split(" : ").next()?.trim();
            // Strip .arch suffix
            let name = name_arch.split('.').next()?.trim().to_string();
            if name.is_empty() { return None; }
            Some(make(pkg, repo, name, "-".to_string()))
        })
        .collect()
}

// zypper -q search → table rows: "| i | name | version | arch | repo | summary |"
// or compact:        "| name | summary |"
fn parse_zypper(pkg: &str, repo: &str, output: &str) -> Vec<PackageInfo> {
    output
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if !l.starts_with('|') { return None; }
            // Split on '|', collect non-empty trimmed fields
            let fields: Vec<&str> = l.split('|')
                .map(|f| f.trim())
                .filter(|f| !f.is_empty())
                .collect();
            // Header rows contain "Name", "S", etc. — skip
            if fields.iter().any(|f| *f == "Name" || *f == "S") { return None; }
            // First meaningful field is the package name
            let name = fields.first()?.trim().to_string();
            if name.is_empty() { return None; }
            // Version is the third field in the full table (S, Name, Version...)
            // but may not be present in compact mode — use "-" as fallback
            let version = if fields.len() >= 3 {
                fields[2].to_string()
            } else {
                "-".to_string()
            };
            Some(make(pkg, repo, name, version))
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
