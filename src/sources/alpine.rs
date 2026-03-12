use super::{PackageInfo, PackageSource, PackageStatus};
use anyhow::{Context, Result};
use scraper::{Html, Selector};

const BASE: &str = "https://pkgs.alpinelinux.org/packages";

pub struct AlpineSource;

/// Map a distq repo identifier to Alpine branch + repo pair.
fn repo_to_branch(repo: &str) -> Option<(&'static str, &'static str)> {
    // Returns (branch, repository) — repository="" means all repos
    match repo {
        "alpine_edge"   => Some(("edge", "")),
        "alpine_3_21"   => Some(("v3.21", "")),
        "alpine_3_20"   => Some(("v3.20", "")),
        "alpine_3_19"   => Some(("v3.19", "")),
        "alpine_3_18"   => Some(("v3.18", "")),
        _               => None,
    }
}

#[async_trait::async_trait]
impl PackageSource for AlpineSource {
    fn name(&self) -> &'static str { "alpine" }

    fn supports(&self, repo: &str) -> bool {
        repo_to_branch(repo).is_some()
    }

    async fn search(
        &self,
        pkg: &str,
        repo: &str,
        client: &reqwest::Client,
    ) -> Result<Vec<PackageInfo>> {
        let (branch, repository) = match repo_to_branch(repo) {
            Some(v) => v,
            None    => return Ok(vec![]),
        };

        let mut query = vec![
            ("name", pkg),
            ("branch", branch),
            ("arch", "x86_64"),
        ];
        if !repository.is_empty() {
            query.push(("repo", repository));
        }

        let html = client
            .get(BASE)
            .query(&query)
            .send()
            .await
            .with_context(|| format!("GET {BASE} name={pkg} branch={branch}"))?
            .text()
            .await
            .context("reading Alpine response")?;

        parse_results(&html, pkg, repo)
    }
}

fn parse_results(html: &str, query_name: &str, repo: &str) -> Result<Vec<PackageInfo>> {
    let doc = Html::parse_document(html);

    // Results table: each <tr> with <td class="package"> is a result row.
    let row_sel     = Selector::parse("table.pure-table tbody tr").unwrap();
    let pkg_sel     = Selector::parse("td.package a").unwrap();
    let ver_sel     = Selector::parse("td.version").unwrap();
    let repo_sel    = Selector::parse("td.repository").unwrap();
    let maint_sel   = Selector::parse("td.maintainer a").unwrap();

    let mut results = Vec::new();
    let mut seen    = std::collections::HashSet::new();

    for row in doc.select(&row_sel) {
        let name = match row.select(&pkg_sel).next() {
            Some(el) => el.text().collect::<String>().trim().to_string(),
            None     => continue,
        };
        let version = row.select(&ver_sel).next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let pkg_repo = row.select(&repo_sel).next()
            .map(|el| el.text().collect::<String>().trim().to_string())
            .unwrap_or_default();
        let maintainer = row.select(&maint_sel).next()
            .map(|el| el.text().collect::<String>().trim().to_string());

        // Deduplicate — same package can appear for multiple arches.
        if !seen.insert((name.clone(), version.clone(), pkg_repo.clone())) {
            continue;
        }

        let maintainers = maintainer.into_iter().collect();

        results.push(PackageInfo {
            query_name:  query_name.to_string(),
            name,
            repo:        repo.to_string(),
            version,
            status:      PackageStatus::Unknown, // Alpine doesn't expose status
            latest:      "-".into(),
            maintainers,
            source:      "alpine",
        });
    }

    Ok(results)
}
