use super::{PackageInfo, PackageSource, PackageStatus};
use anyhow::{Context, Result};
use scraper::{Html, Selector};

// ── Variant table ─────────────────────────────────────────────────────────────

struct Variant {
    source:   &'static str,
    base:     &'static str,
    suite:    &'static str,
    // CSS class on <li> that indicates this suite, e.g. "sid" for unstable.
    li_class: &'static str,
    repo_id:  &'static str,
}

const VARIANTS: &[Variant] = &[
    Variant { source: "debian", base: "https://packages.debian.org", suite: "unstable", li_class: "sid",      repo_id: "debian_unstable" },
    Variant { source: "debian", base: "https://packages.debian.org", suite: "testing",  li_class: "trixie",   repo_id: "debian_testing"  },
    Variant { source: "debian", base: "https://packages.debian.org", suite: "stable",   li_class: "bookworm", repo_id: "debian_stable"   },
    Variant { source: "debian", base: "https://packages.debian.org", suite: "bookworm", li_class: "bookworm", repo_id: "debian_12"       },
    Variant { source: "debian", base: "https://packages.debian.org", suite: "trixie",   li_class: "trixie",   repo_id: "debian_13"       },
    Variant { source: "ubuntu", base: "https://packages.ubuntu.com", suite: "plucky",   li_class: "plucky",   repo_id: "ubuntu_25_04"    },
    Variant { source: "ubuntu", base: "https://packages.ubuntu.com", suite: "oracular", li_class: "oracular", repo_id: "ubuntu_24_10"    },
    Variant { source: "ubuntu", base: "https://packages.ubuntu.com", suite: "noble",    li_class: "noble",    repo_id: "ubuntu_24_04"    },
    Variant { source: "ubuntu", base: "https://packages.ubuntu.com", suite: "jammy",    li_class: "jammy",    repo_id: "ubuntu_22_04"    },
    Variant { source: "ubuntu", base: "https://packages.ubuntu.com", suite: "focal",    li_class: "focal",    repo_id: "ubuntu_20_04"    },
];

fn variant_for(repo: &str) -> Option<&'static Variant> {
    VARIANTS.iter().find(|v| v.repo_id == repo)
}

// ── Sources ───────────────────────────────────────────────────────────────────

pub struct DebianSource;
pub struct UbuntuSource;

#[async_trait::async_trait]
impl PackageSource for DebianSource {
    fn name(&self) -> &'static str { "debian" }
    fn supports(&self, repo: &str) -> bool {
        variant_for(repo).map(|v| v.source == "debian").unwrap_or(false)
    }
    async fn search(&self, pkg: &str, repo: &str, client: &reqwest::Client) -> Result<Vec<PackageInfo>> {
        search_deb(pkg, repo, client).await
    }
}

#[async_trait::async_trait]
impl PackageSource for UbuntuSource {
    fn name(&self) -> &'static str { "ubuntu" }
    fn supports(&self, repo: &str) -> bool {
        variant_for(repo).map(|v| v.source == "ubuntu").unwrap_or(false)
    }
    async fn search(&self, pkg: &str, repo: &str, client: &reqwest::Client) -> Result<Vec<PackageInfo>> {
        search_deb(pkg, repo, client).await
    }
}

// ── Scraper ───────────────────────────────────────────────────────────────────

async fn search_deb(pkg: &str, repo: &str, client: &reqwest::Client) -> Result<Vec<PackageInfo>> {
    let v = match variant_for(repo) {
        Some(v) => v,
        None    => return Ok(vec![]),
    };

    let url = format!("{}/search", v.base);
    let html = client
        .get(&url)
        // packages.debian.org blocks non-browser UAs
        .header("Accept", "text/html,application/xhtml+xml")
        .header("Accept-Language", "en-US,en;q=0.9")
        .query(&[("keywords", pkg), ("searchon", "names"), ("suite", v.suite), ("section", "all")])
        .send().await.with_context(|| format!("GET {url}"))?
        .text().await.context("reading search response")?;

    parse_results(&html, pkg, repo, v)
}

/// Parse Debian/Ubuntu search results page.
///
/// Structure (both sites use the same template):
///
///   <h2>Exact hits</h2>
///   <h3>Package curl</h3>
///   <ul>
///     <li class="sid"><a class="resultlink" href="/sid/curl">sid (unstable)</a> (web):
///       description
///       <br>8.19.0-1: amd64 arm64 ...
///     </li>
///   </ul>
fn parse_results(html: &str, query_name: &str, repo: &str, v: &Variant) -> Result<Vec<PackageInfo>> {
    let doc = Html::parse_document(html);

    // Each package block is an <h3> followed by a <ul>.
    // We need to find the <h3> whose text matches our query, then find the <li>
    // whose class matches the suite.
    let a_sel = Selector::parse("a.resultlink").unwrap();

    // Walk through the document looking for matching package blocks.
    // scraper doesn't support "next sibling" easily, so we collect h3 text + following ul.
    // Instead: select all li elements with the right class and check parent h3.

    // Simpler approach: find all <li class="{suite}"> and check if they're under the right package.
    let li_class_sel = Selector::parse(&format!("li.{}", v.li_class)).unwrap();

    let mut results = Vec::new();

    for li in doc.select(&li_class_sel) {
        // Get the package name from the <a class="resultlink"> inside this <li>.
        let link = match li.select(&a_sel).next() {
            Some(a) => a,
            None    => continue,
        };
        let href = link.value().attr("href").unwrap_or("");
        // href = "/sid/curl" — last segment is package name
        let name = href.rsplit('/').next().unwrap_or("").to_string();
        if name.is_empty() { continue; }

        // For "exact hits" mode we only want the queried package name.
        if name.to_lowercase() != query_name.to_lowercase() { continue; }

        // Version: first text starting with a digit in the <li> after a <br>.
        // The raw text looks like: "\n      8.19.0-1: amd64 arm64 ..."
        let full_text = li.text().collect::<String>();
        let version = extract_version(&full_text).unwrap_or_else(|| "-".into());

        // Maintainer: not available on search page, skip.
        results.push(PackageInfo {
            query_name:  query_name.to_string(),
            name,
            repo:        repo.to_string(),
            version:     version.clone(),
            status:      PackageStatus::Unknown,
            latest:      version,
            maintainers: vec![],
            source:      v.source,
        });

        // One result per query is enough.
        break;
    }

    Ok(results)
}

/// Extract a version string from the raw text of a search result <li>.
///
/// The text looks like:
///   "sid (unstable) (web):\n  description\n  8.19.0-1: amd64 arm64 ...\n  8.19.0~rc3-1 [debports]: ..."
///
/// We want the first version line that:
///   - starts with a digit
///   - contains ':'
///   - does NOT have a '[' before the ':' (those are ports-only entries)
fn extract_version(text: &str) -> Option<String> {
    for line in text.lines() {
        let trimmed = line.trim();
        if !trimmed.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
            continue;
        }
        if !trimmed.contains(':') {
            continue;
        }
        // Extract the part before the first ':'.
        // Skip lines where "[" appears between the start and ":" — those are
        // debports/ports-only entries like "8.19.0-1 [debports]: riscv64".
        // Ubuntu lines look like "8.5.0-2ubuntu10.8 [security]: amd64" —
        // the "[" is after the version, so we strip it and everything after.
        let before_colon = trimmed.split(':').next().unwrap_or("");
        // Remove any "[...]" suffix to get the clean version token.
        let ver_part = before_colon.split('[').next().unwrap_or("").trim();
        // If there was a "[" before the colon with no version before it, skip.
        if ver_part.is_empty() || !ver_part.chars().next().unwrap().is_ascii_digit() {
            continue;
        }
        let ver = ver_part.to_string();
        if !ver.is_empty() {
            return Some(ver);
        }
    }
    None
}
