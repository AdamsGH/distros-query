use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use crate::config::{resolve_dockerfiles_dir, Config};

/// Run `distq docker build [--missing]`.
pub fn run(cfg: &Config, missing_only: bool) -> Result<()> {
    let dir = match resolve_dockerfiles_dir(cfg) {
        Some(d) => d,
        None => bail!(
            "distq: no Dockerfile.* files found.\n\
             Looked next to the binary and in ~/.config/distq/dockerfiles/\n\
             Set [docker] dockerfiles_dir in config, or copy Dockerfiles there."
        ),
    };

    let dockerfiles = collect_dockerfiles(&dir);
    if dockerfiles.is_empty() {
        bail!("distq: no Dockerfile.* files found in {}", dir.display());
    }

    println!("distq: building Docker images from {}", dir.display());
    println!();

    let mut built = 0usize;
    let mut skipped = 0usize;
    let mut failed = 0usize;

    for (distro, path) in &dockerfiles {
        let tag = format!("distq/{distro}");

        if missing_only && image_exists(&tag) {
            println!("  skip    {tag}  (already exists)");
            skipped += 1;
            continue;
        }

        print!("  build   {tag} … ");
        // Flush so the user sees the line before docker starts
        use std::io::Write;
        let _ = std::io::stdout().flush();

        match build_image(path, &tag, &dir) {
            Ok(()) => {
                println!("ok");
                built += 1;
            }
            Err(e) => {
                println!("FAILED");
                eprintln!("          {e}");
                failed += 1;
            }
        }
    }

    println!();
    println!(
        "distq: done — {built} built, {skipped} skipped, {failed} failed"
    );

    if failed > 0 {
        bail!("some images failed to build");
    }
    Ok(())
}

/// Collect all `Dockerfile.<distro>` files in `dir`, sorted by distro name.
fn collect_dockerfiles(dir: &Path) -> Vec<(String, PathBuf)> {
    let mut result: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().into_string().ok()?;
            let distro = name.strip_prefix("Dockerfile.")?.to_string();
            // Skip helper/bench files that aren't distro Dockerfiles
            if distro.is_empty() || distro.contains('.') {
                return None;
            }
            Some((distro, entry.path()))
        })
        .collect();

    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

/// Call `docker build -f <path> -t <tag> <dir>`, streaming output only on failure.
fn build_image(dockerfile: &Path, tag: &str, context_dir: &Path) -> Result<()> {
    let output = Command::new("docker")
        .args([
            "build",
            "-f", &dockerfile.to_string_lossy(),
            "-t", tag,
            &context_dir.to_string_lossy(),
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()?;

    if output.status.success() {
        return Ok(());
    }

    // Print docker's stderr so the user can diagnose the failure.
    let stderr = String::from_utf8_lossy(&output.stderr);
    bail!(
        "docker build exited with {}\n{}",
        output.status,
        stderr.trim()
    );
}

/// Return true if `docker image inspect <tag>` succeeds (image exists locally).
fn image_exists(tag: &str) -> bool {
    Command::new("docker")
        .args(["image", "inspect", "--format", ".", tag])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}
