use super::{PackageInfo, PackageSource, PackageStatus};
use anyhow::{Context, Result};
use serde::Deserialize;

const API: &str = "https://archlinux.org/packages/search/json/";

pub struct ArchSource;

#[async_trait::async_trait]
impl PackageSource for ArchSource {
    fn name(&self) -> &'static str { "arch" }

    fn supports(&self, repo: &str) -> bool {
        matches!(repo, "arch" | "arch_testing" | "archlinux")
    }

    async fn search(
        &self,
        pkg: &str,
        repo: &str,
        client: &reqwest::Client,
    ) -> Result<Vec<PackageInfo>> {
        #[derive(Deserialize)]
        struct Response {
            results: Vec<ArchPackage>,
        }

        #[derive(Deserialize)]
        struct ArchPackage {
            pkgname: String,
            pkgver:  String,
            pkgrel:  String,
            #[allow(dead_code)]
            pkgdesc: String,
            repo:    String,
            maintainers: Vec<String>,
            flag_date: Option<String>,
        }

        let resp: Response = client
            .get(API)
            .query(&[("name", pkg)])
            .send()
            .await
            .with_context(|| format!("GET {API} name={pkg}"))?
            .json()
            .await
            .context("parsing Arch API response")?;

        // Find the latest version from all results (first non-testing result).
        let latest = resp.results.iter()
            .find(|p| p.repo != "testing")
            .or_else(|| resp.results.first())
            .map(|p| format!("{}-{}", p.pkgver, p.pkgrel))
            .unwrap_or_else(|| "-".into());

        Ok(resp.results
            .into_iter()
            .map(|p| {
                let version = format!("{}-{}", p.pkgver, p.pkgrel);
                // flagged = out of date
                let status = if p.flag_date.is_some() {
                    PackageStatus::Outdated
                } else if version == latest {
                    PackageStatus::Newest
                } else {
                    PackageStatus::Legacy
                };
                PackageInfo {
                    query_name:  pkg.to_string(),
                    name:        p.pkgname,
                    repo:        repo.to_string(),
                    version,
                    status,
                    latest:      latest.clone(),
                    maintainers: p.maintainers,
                    source:      "arch",
                }
            })
            .collect())
    }
}
