use std::collections::HashMap;

/// Detect the current OS and return a Repology repository identifier.
pub fn detect() -> Option<String> {
    detect_platform()
}

#[cfg(target_os = "linux")]
fn detect_platform() -> Option<String> {
    from_os_release()
}

#[cfg(target_os = "freebsd")]
fn detect_platform() -> Option<String> {
    Some("freebsd".into())
}

#[cfg(target_os = "netbsd")]
fn detect_platform() -> Option<String> {
    Some("pkgsrc_current".into())
}

#[cfg(target_os = "openbsd")]
fn detect_platform() -> Option<String> {
    Some("openbsd".into())
}

#[cfg(target_os = "macos")]
fn detect_platform() -> Option<String> {
    Some("homebrew".into())
}

#[cfg(target_os = "windows")]
fn detect_platform() -> Option<String> {
    Some("chocolatey".into())
}

#[cfg(not(any(
    target_os = "linux",
    target_os = "freebsd",
    target_os = "netbsd",
    target_os = "openbsd",
    target_os = "macos",
    target_os = "windows",
)))]
fn detect_platform() -> Option<String> {
    None
}

#[cfg(target_os = "linux")]
fn from_os_release() -> Option<String> {
    let content = std::fs::read_to_string("/etc/os-release").ok()?;
    let fields = parse_os_release(&content);

    let id = fields.get("ID").map(|s| s.as_str()).unwrap_or("").to_lowercase();
    let id_like = fields.get("ID_LIKE").map(|s| s.as_str()).unwrap_or("").to_lowercase();
    let version_id = fields.get("VERSION_ID").map(|s| s.as_str()).unwrap_or("");
    let version_codename = fields.get("VERSION_CODENAME").map(|s| s.as_str()).unwrap_or("");

    map_distro(&id, &id_like, version_id, version_codename)
}

fn parse_os_release(content: &str) -> HashMap<String, String> {
    content
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }
            let (k, v) = line.split_once('=')?;
            let v = v.trim_matches('"').to_string();
            Some((k.to_string(), v))
        })
        .collect()
}

fn map_distro(id: &str, id_like: &str, version_id: &str, _codename: &str) -> Option<String> {
    match id {
        "alpine" => Some("alpine_edge".into()),
        "arch" | "archlinux" => Some("arch".into()),
        "artix" => Some("artix".into()),
        "centos" => {
            let major = version_id.split('.').next().unwrap_or("").to_string();
            if major.is_empty() {
                Some("centos".into())
            } else {
                Some(format!("centos_{major}"))
            }
        }
        "debian" => Some("debian_unstable".into()),
        "fedora" => Some("fedora_rawhide".into()),
        "gentoo" => Some("gentoo".into()),
        "guix" => Some("gnu_guix".into()),
        "mageia" => Some("mageia_cauldron".into()),
        "manjaro" => Some("manjaro_stable".into()),
        "nixos" => Some("nix_unstable".into()),
        "opensuse-leap" => {
            // e.g. version_id = "15.6" → "opensuse_leap_15_6"
            let v = version_id.replace('.', "_");
            Some(format!("opensuse_leap_{v}"))
        }
        "opensuse-tumbleweed" | "opensuse" => Some("opensuse_tumbleweed".into()),
        "rhel" => {
            let major = version_id.split('.').next().unwrap_or("").to_string();
            if major.is_empty() {
                Some("rhel".into())
            } else {
                Some(format!("rhel_{major}"))
            }
        }
        "slackware" => Some("slackware_current".into()),
        "ubuntu" => {
            // version_id = "24.04" → "ubuntu_24_04"
            let v = version_id.replace('.', "_");
            if v.is_empty() {
                Some("ubuntu".into())
            } else {
                Some(format!("ubuntu_{v}"))
            }
        }
        "void" => Some("void_x86_64".into()),
        _ => {
            // Fall back to ID_LIKE for derivative distros
            for like in id_like.split_whitespace() {
                if let Some(r) = map_distro(like, "", version_id, _codename) {
                    return Some(r);
                }
            }
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ubuntu() {
        assert_eq!(map_distro("ubuntu", "", "24.04", ""), Some("ubuntu_24_04".into()));
        assert_eq!(map_distro("ubuntu", "", "22.10", ""), Some("ubuntu_22_10".into()));
    }

    #[test]
    fn test_opensuse() {
        assert_eq!(map_distro("opensuse-leap", "", "15.6", ""), Some("opensuse_leap_15_6".into()));
        assert_eq!(map_distro("opensuse-tumbleweed", "", "", ""), Some("opensuse_tumbleweed".into()));
    }

    #[test]
    fn test_id_like_fallback() {
        // Mint has ID_LIKE=ubuntu
        assert_eq!(map_distro("linuxmint", "ubuntu", "24.04", ""), Some("ubuntu_24_04".into()));
    }

    #[test]
    fn test_arch() {
        assert_eq!(map_distro("arch", "", "", ""), Some("arch".into()));
        assert_eq!(map_distro("manjaro", "", "", ""), Some("manjaro_stable".into()));
    }
}
