use super::{PackageInfo, PackageSource, PackageStatus};
use anyhow::{Context, Result};
use serde::Deserialize;

const MDAPI: &str = "https://mdapi.fedoraproject.org";

pub struct FedoraSource;

/// Map a distq repo identifier to an mdapi branch name.
fn repo_to_branch(repo: &str) -> Option<&'static str> {
    match repo {
        "fedora_rawhide" | "fedora"  => Some("rawhide"),
        "fedora_41"                  => Some("f41"),
        "fedora_42"                  => Some("f42"),
        "fedora_40"                  => Some("f40"),
        _                            => None,
    }
}

#[async_trait::async_trait]
impl PackageSource for FedoraSource {
    fn name(&self) -> &'static str { "fedora" }

    fn supports(&self, repo: &str) -> bool {
        repo_to_branch(repo).is_some()
    }

    async fn search(
        &self,
        pkg: &str,
        repo: &str,
        client: &reqwest::Client,
    ) -> Result<Vec<PackageInfo>> {
        let branch = match repo_to_branch(repo) {
            Some(b) => b,
            None    => return Ok(vec![]),
        };

        #[derive(Deserialize)]
        struct MdapiPkg {
            basename: Option<String>,
            version:  String,
            #[allow(dead_code)]
            release:  String,
            #[allow(dead_code)]
            summary:  Option<String>,
        }

        let url = format!("{MDAPI}/{branch}/pkg/{pkg}");
        let resp = client.get(&url).send().await
            .with_context(|| format!("GET {url}"))?;

        let status = resp.status();
        if status.as_u16() == 404 || status.as_u16() == 400 {
            return Ok(vec![]);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("mdapi error {status}: {body}");
        }

        let p: MdapiPkg = resp.json().await.context("parsing mdapi response")?;

        let name = p.basename.unwrap_or_else(|| pkg.to_string());
        let version = p.version.clone();

        Ok(vec![PackageInfo {
            query_name:  pkg.to_string(),
            name,
            repo:        repo.to_string(),
            version:     version.clone(),
            status:      PackageStatus::Newest, // mdapi returns the current package, assumed newest
            latest:      version,
            maintainers: vec![],
            source:      "fedora",
        }])
    }
}
