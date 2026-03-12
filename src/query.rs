use anyhow::{Context, Result, bail};
use serde::de::DeserializeOwned;
use serde::Deserialize;

const API_BASE: &str = "https://repology.org/api/v1";
/// Maximum number of retry attempts on 429 / 5xx responses.
const MAX_RETRIES: u32 = 4;

/// GET with exponential backoff on 429 Too Many Requests and 5xx errors.
/// Waits 1s, 2s, 4s, 8s between attempts.
async fn get_with_retry<T: DeserializeOwned>(
    client: &reqwest::Client,
    url: &str,
    query: &[(String, String)],
) -> Result<T> {
    let mut delay_ms: u64 = 1_000;

    for attempt in 0..=MAX_RETRIES {
        let resp = client
            .get(url)
            .query(query)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;

        let status = resp.status();

        if status.is_success() {
            return resp
                .json::<T>()
                .await
                .with_context(|| format!("parsing JSON from {url}"));
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

/// A single package entry returned by the API.
#[derive(Debug, Deserialize, Clone)]
pub struct ApiPackage {
    pub repo: String,
    pub srcname: Option<String>,
    pub binname: Option<String>,
    pub visiblename: Option<String>,
    pub version: String,
    pub status: Option<String>,
    pub maintainers: Option<Vec<String>>,
}

/// A processed, display-ready package record.
#[derive(Debug, Clone)]
pub struct PackageInfo {
    /// The project name as originally queried (used for column grouping in transposed view).
    pub query_name: String,
    /// Source or binary name used for display/sorting.
    pub name: String,
    pub repo: String,
    pub version: String,
    pub status: PackageStatus,
    /// The latest known version across all repos for this project.
    pub latest: String,
    pub maintainers: Vec<String>,
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
            "newest" => Self::Newest,
            "outdated" => Self::Outdated,
            "devel" => Self::Devel,
            "legacy" => Self::Legacy,
            "rolling" => Self::Rolling,
            "unique" => Self::Unique,
            "noscheme" => Self::NoScheme,
            "incorrect" => Self::Incorrect,
            "untrusted" => Self::Untrusted,
            "ignored" => Self::Ignored,
            _ => Self::Unknown,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Newest => "newest",
            Self::Outdated => "outdated",
            Self::Devel => "devel",
            Self::Legacy => "legacy",
            Self::Rolling => "rolling",
            Self::Unique => "unique",
            Self::NoScheme => "noscheme",
            Self::Incorrect => "incorrect",
            Self::Untrusted => "untrusted",
            Self::Ignored => "ignored",
            Self::Unknown => "unknown",
        }
    }


}

/// Determine the "latest" version from a list of API packages.
fn find_latest(packages: &[ApiPackage]) -> String {
    for pkg in packages {
        let status = pkg.status.as_deref().unwrap_or("");
        match status {
            "newest" => return pkg.version.clone(),
            _ => {}
        }
    }
    for pkg in packages {
        let status = pkg.status.as_deref().unwrap_or("");
        match status {
            "devel" | "unique" => return pkg.version.clone(),
            _ => {}
        }
    }
    for pkg in packages {
        let status = pkg.status.as_deref().unwrap_or("");
        if status == "noscheme" {
            return "noscheme".into();
        }
        if status == "rolling" {
            return "rolling".into();
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

    // Deduplicate: when sort_package is off we use srcname, so multiple binary
    // packages from the same source produce identical rows — keep only the first.
    let mut seen = std::collections::HashSet::new();

    api_packages
        .into_iter()
        .filter(|p| all || p.repo == repo)
        .filter_map(|p| {
            let name = if all {
                p.visiblename
                    .clone()
                    .or_else(|| p.srcname.clone())
                    .unwrap_or_else(|| p.repo.clone())
            } else if sort_package || is_chocolatey {
                p.binname
                    .clone()
                    .or_else(|| p.srcname.clone())
                    .unwrap_or_default()
            } else {
                p.srcname
                    .clone()
                    .or_else(|| p.binname.clone())
                    .unwrap_or_default()
            };

            let repo_name = if all {
                p.repo.clone()
            } else {
                repo.to_string()
            };

            let dedup_key = (repo_name.clone(), name.clone(), p.version.clone());
            if !seen.insert(dedup_key) {
                return None;
            }

            let status = PackageStatus::from_str(p.status.as_deref().unwrap_or(""));
            let maintainers = p.maintainers.unwrap_or_default();

            Some(PackageInfo {
                query_name: String::new(), // filled in by callers that know the query
                name,
                repo: repo_name,
                version: p.version,
                status,
                latest: latest.clone(),
                maintainers,
            })
        })
        .collect()
}

/// Fetch a single named package: GET /api/v1/project/<name>
pub async fn fetch_package(
    client: &reqwest::Client,
    package: &str,
    repo: &str,
    all: bool,
    sort_package: bool,
    query: &[(String, String)],
) -> Result<Vec<PackageInfo>> {
    let url = format!("{API_BASE}/project/{package}");

    // For multi-repo queries the caller doesn't set inrepo; add it here per-request.
    let mut effective_query = query.to_vec();
    if !all && !repo.is_empty() && !query.iter().any(|(k, _)| k == "inrepo") {
        effective_query.push(("inrepo".into(), repo.into()));
    }

    let packages: Vec<ApiPackage> = get_with_retry(client, &url, &effective_query)
        .await
        .with_context(|| format!("fetching package '{package}'"))?;

    let mut results = process_packages(packages, repo, all, sort_package);
    // Tag every result with the original query name for transposed view grouping.
    for r in &mut results {
        r.query_name = package.to_string();
    }
    Ok(results)
}

/// Fetch a paginated list: GET /api/v1/projects/[<begin>/]
/// Returns up to 200 results per call. Pass `begin` to start from a specific project name.
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
        (None, None) => format!("{API_BASE}/projects/"),
    };

    // /api/v1/projects/ returns { "projectname": [ ...packages... ], ... }
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

/// Fetch multiple pages automatically by chaining `begin` cursors.
/// Returns at most `pages * 200` results.
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
        let page = fetch_packages_list(
            client,
            repo,
            begin.as_deref(),
            None,
            query.clone(),
        )
        .await?;

        if page.is_empty() {
            break;
        }

        // The last project name is the cursor for the next page.
        begin = page.last().map(|p| p.name.clone());
        all_results.extend(page);
    }

    Ok(all_results)
}
