# distq

CLI for querying package names and versions across Linux distributions. Answers the question: *"what is this package called in apt / pacman / dnf / apk?"*

```
$ distq --profile linux curl

┌────────────────────────────────────────────────────────────┐
│ SOURCE   REPO                  curl                        │
╞════════════════════════════════════════════════════════════╡
│ alpine   alpine_edge           curl 8.19.0-r0              │
│ docker   arch                  curl 8.19.0-1               │
│ docker   debian_unstable       curl -                      │
│ docker   fedora_rawhide        curl -                      │
│ nixos    nix_unstable          curl 8.18.0                 │
│ docker   opensuse_tumbleweed   curl -                      │
│ docker   ubuntu                curl -                      │
│ docker   void_x86_64           curl 8.18.0_1               │
└────────────────────────────────────────────────────────────┘
```

## How it works

distq queries packages from multiple sources in priority order. For each repo it picks the first source that can handle it and returns results:

```
docker -> arch -> aur -> fedora -> alpine -> debian -> ubuntu -> nixos -> repology
```

**Native sources** scrape distro APIs/websites directly:

| Source | Repos | Method |
|--------|-------|--------|
| `arch` | `arch` | pkgs.archlinux.org API |
| `aur` | `aur` | aur.archlinux.org RPC v5 |
| `fedora` | `fedora_*` | mdapi.fedoraproject.org |
| `alpine` | `alpine_*` | pkgs.alpinelinux.org HTML |
| `debian` | `debian_*` | packages.debian.org |
| `ubuntu` | `ubuntu_*` | packages.ubuntu.com |
| `nixos` | `nix_*` | local JSON cache (24h TTL, ~9 MB brotli) |
| `repology` | any | repology.org API (universal fallback) |

**Docker source** runs each distro's own package manager in an isolated minimal image, with the package database pre-cached at build time. This gives you native `pacman`, `apt-cache`, `dnf5`, `zypper`, `xbps-query`, and `apk` without installing them.

If a non-repology source returns empty results, distq falls through to the next matching source before falling back to Repology.

## Installation

### Pre-built binary

Download from [Releases](../../releases). The archive contains the `distq` binary and a `dockerfiles/` directory.

### From source

```sh
cargo install --path .
```

Requires Rust 1.85+ (edition 2024).

## Docker images

The docker source uses images tagged `distq/<distro>`. Each image contains the distro's own package manager binary plus a pre-cached package database, so no network access is needed at query time.

| Image | Package manager | Base distro |
|-------|-----------------|-------------|
| `distq/arch` | `pacman` | archlinux:base |
| `distq/debian` | `apt-cache` | debian:bookworm-slim |
| `distq/ubuntu` | `apt-cache` | ubuntu:24.04 |
| `distq/fedora` | `dnf5` | fedora:42 |
| `distq/opensuse` | `zypper` | opensuse/leap:15.5 |
| `distq/void` | `xbps-query` | voidlinux/voidlinux |
| `distq/alpine` | `apk` | alpine:3.19 |

```sh
# Build all images
distq docker build

# Build only images not yet present locally
distq docker build --missing

# List built images with sizes
distq docker list

# Build a single image manually
just docker-one arch
```

Dockerfiles are in the `dockerfiles/` directory. Set `dockerfiles_dir` in config to point distq at them if the binary and dockerfiles are in different locations.

## Usage

### Basic query

```sh
# Single package, autodetect current distro's repo
distq curl

# Specific repo
distq --repo arch curl

# Multiple repos
distq --repos arch,debian,fedora,alpine curl

# Multiple packages at once
distq --repos arch,debian curl wget htop
```

### Profiles

Profiles are named sets of repos. Built-in: `linux`, `bsd`, `all`.

```sh
distq --profile linux curl
```

### Output formats

```sh
# Default: transposed table (repo-per-row) for multi-repo, flat for single-repo
distq --repos arch,debian curl

# Force flat layout
distq --repos arch,debian --layout flat curl

# JSON (includes source field, suitable for piping)
distq --repos arch,debian --format json curl | jq '.[].name'
```

### Force a specific source backend

```sh
distq --source docker curl
distq --source repology curl
```

### Listing mode

Without a package argument distq switches to listing/browsing mode via Repology:

```sh
distq --repo arch
distq --repo arch --page 3
distq --repo arch --newest
distq --repo arch --outdated
distq --repo arch --maintainer user@example.com
distq --repo debian --search "python"
```

### Environment variables

| Variable | Effect |
|----------|--------|
| `DISTQ_REPOS` | Override default repo list (comma-separated) |
| `NO_COLOR` | Disable colour output |

## Configuration

```sh
distq --init-config
```

Writes `~/.config/distq/config.toml`:

```toml
default_repos = []        # leave empty to autodetect from current distro

[profiles]
linux = ["arch", "aur", "debian", "ubuntu", "fedora", "alpine", "nixos", "void", "opensuse"]
bsd   = ["freebsd", "openbsd", "netbsd"]

[sources]
priority = ["docker", "arch", "aur", "fedora", "alpine", "debian", "ubuntu", "nixos", "repology"]

[docker]
# dockerfiles_dir = "/path/to/dockerfiles"
```

The `priority` list controls which source is tried first for each repo. The first source that matches and returns non-empty results wins. `repology` is always appended as a last-resort fallback even if not listed.

## Output details

### Transposed table (default for multi-repo)

```
┌──────────────────────────────────┐
│ SOURCE   REPO     curl   wget    │
╞══════════════════════════════════╡
│ arch     arch     8.19   1.24    │
│ docker   debian   -      1.21    │
└──────────────────────────────────┘
```

### Flat table (default for single-repo, or `--layout flat`)

```
┌─────────────────────────────────────────┐
│ REPO   PACKAGE   VERSION   STATUS       │
╞═════════════════════════════════════════╡
│ arch   curl      8.19.0-1  newest       │
│ arch   libcurl   8.19.0-1  newest       │
└─────────────────────────────────────────┘
```

### JSON

```json
[
  {
    "repo": "arch",
    "name": "curl",
    "version": "8.19.0-1",
    "status": "newest",
    "latest": "-",
    "maintainers": [],
    "source": "arch"
  }
]
```

## Development

```sh
just run curl        # build debug and run
just smoke           # search 'curl' across the linux profile
just test
just lint
just package         # build release + pack into dist/
just tag-release     # tag from Cargo.toml version and push
just tag-release 0.2.0  # bump Cargo.toml, tag, push
```

## Architecture

```
src/
  main.rs          CLI entry point, argument parsing (clap), dispatch
  query.rs         Core query engine: source routing, parallelism, fallback chain
  format.rs        Table and JSON rendering (comfy-table)
  config.rs        Config file (TOML), profile resolution, docker config
  autodetect.rs    Detect current distro from /etc/os-release + ID_LIKE fallback
  docker_build.rs  distq docker build/list implementation
  sources/
    mod.rs         PackageSource trait, registry, DEFAULT_PRIORITY
    arch.rs        pkgs.archlinux.org
    aur.rs         aur.archlinux.org RPC v5
    fedora.rs      mdapi.fedoraproject.org
    alpine.rs      pkgs.alpinelinux.org (HTML scrape)
    debian.rs      packages.debian.org + packages.ubuntu.com
    nixos.rs       local JSON cache, brotli-compressed, 24h TTL
    docker.rs      docker run distq/<distro>, per-distro output parsers
    repology.rs    repology.org API, pagination, retry on 429
dockerfiles/
  Dockerfile.{arch,debian,ubuntu,fedora,opensuse,void,alpine}
```

### Adding a new source

1. Create `src/sources/<name>.rs` implementing `PackageSource`
2. Register it in `src/sources/mod.rs` (`ordered_sources`, `single_source`, `DEFAULT_PRIORITY`)
3. Add `pub mod <name>;` to `mod.rs`

### Adding a new docker image

1. Write `dockerfiles/Dockerfile.<distro>` -- the image ENTRYPOINT should accept a search term and print results to stdout
2. Add a parser in `src/sources/docker.rs` (`parser_for`, `ParseMode`)
3. Map the repo name to the image in `DockerSource::supports()` and `search()`
