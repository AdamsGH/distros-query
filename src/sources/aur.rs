use super::{PackageInfo, PackageSource, PackageStatus};
use anyhow::{Context, Result};
use serde::Deserialize;

const API: &str = "https://aur.archlinux.org/rpc/v5/info";

pub struct AurSource;

#[async_trait::async_trait]
impl PackageSource for AurSource {
    fn name(&self) -> &'static str { "aur" }

    fn supports(&self, repo: &str) -> bool {
        matches!(repo, "aur")
    }

    async fn search(
        &self,
        pkg: &str,
        _repo: &str,
        client: &reqwest::Client,
    ) -> Result<Vec<PackageInfo>> {
        #[derive(Deserialize)]
        struct Response {
            results: Vec<AurPackage>,
        }

        #[derive(Deserialize)]
        #[serde(rename_all = "PascalCase")]
        struct AurPackage {
            name:        String,
            version:     String,
            maintainer:  Option<String>,
            out_of_date: Option<i64>,
        }

        let resp: Response = client
            .get(API)
            // AUR info endpoint accepts exact name lookup via ?arg[]=<name>
            .query(&[("arg[]", pkg)])
            .send()
            .await
            .with_context(|| format!("GET {API} arg[]={pkg}"))?
            .json()
            .await
            .context("parsing AUR API response")?;

        let latest = resp.results.first()
            .map(|p| p.version.clone())
            .unwrap_or_else(|| "-".into());

        Ok(resp.results
            .into_iter()
            .map(|p| {
                let status = if p.out_of_date.is_some() {
                    PackageStatus::Outdated
                } else {
                    PackageStatus::Unique  // AUR packages are always "unique" distro-wise
                };
                let maintainers = p.maintainer.into_iter().collect();
                PackageInfo {
                    query_name:  pkg.to_string(),
                    name:        p.name,
                    repo:        "aur".to_string(),
                    version:     p.version,
                    status,
                    latest:      latest.clone(),
                    maintainers,
                    source:      "aur",
                }
            })
            .collect())
    }
}
