use super::{PackageInfo, PackageSource, PackageStatus};
use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::Deserialize;

const API_BASE: &str = "https://repology.org/api/v1";
const MAX_RETRIES: u32 = 4;

pub struct RepologySource {
    sort_package: bool,
}

impl RepologySource {
    pub fn new() -> Self {
        Self { sort_package: false }
    }

    #[allow(dead_code)]
    pub fn with_sort_package(mut self, v: bool) -> Self {
        self.sort_package = v;
        self
    }
}

#[async_trait::async_trait]
impl PackageSource for RepologySource {
    fn name(&self) -> &'static str { "repology" }

    // Repology covers everything — universal fallback.
    fn supports(&self, _repo: &str) -> bool { true }

    async fn search(
        &self,
        pkg: &str,
        repo: &str,
        client: &reqwest::Client,
    ) -> Result<Vec<PackageInfo>> {
        fetch_package(client, pkg, repo, false, self.sort_package, &[]).await
    }
}

// ── Public helpers used by main for listing mode ──────────────────────────────

/// Fetch a single named package: GET /api/v1/project/<name>
pub async fn fetch_package(
    client: &reqwest::Client,
    package: &str,
    repo: &str,
    all: bool,
    sort_package: bool,
    extra_query: &[(String, String)],
) -> Result<Vec<PackageInfo>> {
    let url = format!("{API_BASE}/project/{package}");

    let mut query = extra_query.to_vec();
    if !all && !repo.is_empty() && !query.iter().any(|(k, _)| k == "inrepo") {
        query.push(("inrepo".into(), repo.into()));
    }

    let packages: Vec<ApiPackage> = get_with_retry(client, &url, &query)
        .await
        .with_context(|| format!("fetching package '{package}'"))?;

    let mut results = process_packages(packages, repo, all, sort_package);
    for r in &mut results {
        r.query_name = package.to_string();
    }
    Ok(results)
}

/// Fetch a paginated project listing: GET /api/v1/projects/[<begin>/]
pub async fn fetch_packages_list(
    client: &reqwest::Client,
    repo: &str,
    begin: Option<&str>,
    end: Option<&str>,
    query: Vec<(String, String)>,
) -> Result<Vec<PackageInfo>> {
    let path = match (begin, end) {
        (Some(b), _) => format!("{API_BASE}/projects/{b}/"),
        (None, Some(e)) => format!("{API_BASE}/projects/..{e}/"),
        (None, None)    => format!("{API_BASE}/projects/"),
    };

    let map: serde_json::Map<String, serde_json::Value> =
        get_with_retry(client, &path, &query)
            .await
            .context("fetching project list")?;

    let mut results = Vec::new();
    for (_project, value) in map {
        let api_packages: Vec<ApiPackage> = serde_json::from_value(value)
            .context("failed to parse package array")?;
        results.extend(process_packages(api_packages, repo, false, false));
    }
    results.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(results)
}

/// Fetch multiple pages by chaining begin-cursors.
pub async fn fetch_pages(
    client: &reqwest::Client,
    repo: &str,
    start: Option<&str>,
    pages: u32,
    query: Vec<(String, String)>,
) -> Result<Vec<PackageInfo>> {
    let mut all_results = Vec::new();
    let mut begin: Option<String> = start.map(str::to_string);

    for _ in 0..pages {
        let page = fetch_packages_list(client, repo, begin.as_deref(), None, query.clone()).await?;
        if page.is_empty() { break; }
        begin = page.last().map(|p| p.name.clone());
        all_results.extend(page);
    }
    Ok(all_results)
}

// ── Internal ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize, Clone)]
struct ApiPackage {
    repo: String,
    srcname: Option<String>,
    binname: Option<String>,
    visiblename: Option<String>,
    version: String,
    status: Option<String>,
    maintainers: Option<Vec<String>>,
}

fn find_latest(packages: &[ApiPackage]) -> String {
    for pkg in packages {
        if pkg.status.as_deref() == Some("newest") { return pkg.version.clone(); }
    }
    for pkg in packages {
        match pkg.status.as_deref() {
            Some("devel") | Some("unique") => return pkg.version.clone(),
            _ => {}
        }
    }
    for pkg in packages {
        match pkg.status.as_deref() {
            Some("noscheme") => return "noscheme".into(),
            Some("rolling")  => return "rolling".into(),
            _ => {}
        }
    }
    "-".into()
}

fn process_packages(
    api_packages: Vec<ApiPackage>,
    repo: &str,
    all: bool,
    sort_package: bool,
) -> Vec<PackageInfo> {
    let latest = find_latest(&api_packages);
    let is_chocolatey = repo == "chocolatey";
    let mut seen = std::collections::HashSet::new();

    api_packages
        .into_iter()
        .filter(|p| all || p.repo == repo)
        .filter_map(|p| {
            let name = if all {
                p.visiblename.clone().or_else(|| p.srcname.clone()).unwrap_or_else(|| p.repo.clone())
            } else if sort_package || is_chocolatey {
                p.binname.clone().or_else(|| p.srcname.clone()).unwrap_or_default()
            } else {
                p.srcname.clone().or_else(|| p.binname.clone()).unwrap_or_default()
            };

            let repo_name = if all { p.repo.clone() } else { repo.to_string() };
            if !seen.insert((repo_name.clone(), name.clone(), p.version.clone())) {
                return None;
            }

            let status = PackageStatus::from_str(p.status.as_deref().unwrap_or(""));
            Some(PackageInfo {
                query_name:  String::new(),
                name,
                repo:        repo_name,
                version:     p.version,
                status,
                latest:      latest.clone(),
                maintainers: p.maintainers.unwrap_or_default(),
                source:      "repology",
            })
        })
        .collect()
}

async fn get_with_retry<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    query: &[(String, String)],
) -> Result<T> {
    let mut delay_ms: u64 = 1_000;
    for attempt in 0..=MAX_RETRIES {
        let resp = client.get(url).query(query).send().await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status();
        if status.is_success() {
            return resp.json::<T>().await.with_context(|| format!("parsing JSON from {url}"));
        }
        if (status.as_u16() == 429 || status.is_server_error()) && attempt < MAX_RETRIES {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            delay_ms *= 2;
            continue;
        }
        let body = resp.text().await.unwrap_or_default();
        bail!("API error {status}: {body}");
    }
    unreachable!()
}
