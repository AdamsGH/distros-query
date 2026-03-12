use super::{PackageInfo, PackageSource, PackageStatus};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io::Read;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

// Cache TTL: 24 hours — NixOS unstable gets new packages daily but
// we don't need to download 9MB brotli on every query.
const CACHE_TTL: Duration = Duration::from_secs(60 * 60 * 24);

const CHANNELS: &[(&str, &str, &str)] = &[
    // (repo_id, channel_name, url)
    ("nixos",          "nixos-unstable", "https://channels.nixos.org/nixos-unstable/packages.json.br"),
    ("nix_unstable",   "nixos-unstable", "https://channels.nixos.org/nixos-unstable/packages.json.br"),
    ("nixos_unstable", "nixos-unstable", "https://channels.nixos.org/nixos-unstable/packages.json.br"),
    ("nixos_24_11",    "nixos-24.11",    "https://channels.nixos.org/nixos-24.11/packages.json.br"),
    ("nixos_24_05",    "nixos-24.05",    "https://channels.nixos.org/nixos-24.05/packages.json.br"),
];

fn channel_for(repo: &str) -> Option<(&'static str, &'static str)> {
    CHANNELS.iter()
        .find(|(id, _, _)| *id == repo)
        .map(|(_, name, url)| (*name, *url))
}

pub struct NixosSource;

#[async_trait::async_trait]
impl PackageSource for NixosSource {
    fn name(&self) -> &'static str { "nixos" }

    fn supports(&self, repo: &str) -> bool {
        channel_for(repo).is_some()
    }

    async fn search(
        &self,
        pkg: &str,
        repo: &str,
        client: &reqwest::Client,
    ) -> Result<Vec<PackageInfo>> {
        let (channel, url) = match channel_for(repo) {
            Some(v) => v,
            None    => return Ok(vec![]),
        };

        let index = load_or_fetch(client, channel, url).await
            .with_context(|| format!("loading NixOS package index for {channel}"))?;

        Ok(search_index(&index, pkg, repo))
    }
}

// ── Index loading ─────────────────────────────────────────────────────────────

#[derive(Deserialize, Serialize)]
struct NixIndex {
    packages: HashMap<String, NixPackage>,
}

#[derive(Deserialize, Serialize)]
struct NixPackage {
    pname:   String,
    version: String,
    meta:    NixMeta,
}

#[derive(Deserialize, Serialize)]
struct NixMeta {
    #[serde(default)]
    maintainers: Vec<NixMaintainer>,
}

#[derive(Deserialize, Serialize)]
struct NixMaintainer {
    #[serde(default)]
    name:  String,
    #[serde(default)]
    email: String,
}

/// Load the package index from local cache, or fetch + decompress + cache it.
async fn load_or_fetch(
    client: &reqwest::Client,
    channel: &str,
    url: &str,
) -> Result<NixIndex> {
    let cache_path = cache_path(channel);

    // Check if cache is fresh enough.
    if let Ok(meta) = std::fs::metadata(&cache_path) {
        if let Ok(modified) = meta.modified() {
            if SystemTime::now().duration_since(modified).unwrap_or(CACHE_TTL) < CACHE_TTL {
                if let Ok(data) = std::fs::read_to_string(&cache_path) {
                    if let Ok(index) = serde_json::from_str::<NixIndex>(&data) {
                        return Ok(index);
                    }
                }
            }
        }
    }

    eprintln!("distq: fetching NixOS package index for {channel} (~9MB, cached for 24h)...");

    // Download brotli-compressed JSON.
    let bytes = client
        .get(url)
        .send().await.with_context(|| format!("GET {url}"))?
        .bytes().await.context("reading NixOS index")?;

    // Decompress brotli → raw JSON bytes.
    let mut decompressed = Vec::with_capacity(bytes.len() * 20);
    let mut reader = brotli::Decompressor::new(bytes.as_ref(), 4096);
    reader.read_to_end(&mut decompressed).context("decompressing NixOS index")?;

    // Parse JSON.
    let index: NixIndex = serde_json::from_slice(&decompressed)
        .context("parsing NixOS package index")?;

    // Save decompressed JSON to cache (next lookup is instant).
    if let Some(dir) = cache_path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    // Write only pname+version+maintainers to keep cache small (~30MB → ~8MB).
    // We do this by re-serialising just what we need.
    let _ = save_cache(&cache_path, &index);

    Ok(index)
}

fn save_cache(path: &PathBuf, index: &NixIndex) -> Result<()> {
    // Serialize the full index — serde handles it.
    // In practice the cache is ~50MB but lives in ~/.cache and is read in <1s.
    let json = serde_json::to_string(index).context("serializing cache")?;
    std::fs::write(path, json).with_context(|| format!("writing cache {}", path.display()))
}

fn cache_path(channel: &str) -> PathBuf {
    dirs_next::cache_dir()
        .unwrap_or_else(|| PathBuf::from("/tmp"))
        .join("distq")
        .join(format!("nixos-{channel}.json"))
}

// ── Search ────────────────────────────────────────────────────────────────────

fn search_index(index: &NixIndex, pkg: &str, repo: &str) -> Vec<PackageInfo> {
    let pkg_lower = pkg.to_lowercase();
    let mut results = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for (_attr, p) in &index.packages {
        if p.pname.to_lowercase() != pkg_lower {
            continue;
        }
        // Deduplicate by (pname, version) — same package exists for multiple systems.
        if !seen.insert((p.pname.clone(), p.version.clone())) {
            continue;
        }

        let maintainers = p.meta.maintainers.iter()
            .map(|m| {
                if m.email.is_empty() { m.name.clone() }
                else { format!("{} <{}>", m.name, m.email) }
            })
            .collect();

        results.push(PackageInfo {
            query_name:  pkg.to_string(),
            name:        p.pname.clone(),
            repo:        repo.to_string(),
            version:     p.version.clone(),
            status:      PackageStatus::Unknown,
            latest:      p.version.clone(),
            maintainers,
            source:      "nixos",
        });
    }

    results
}
