default:
    @just --list

# ── Build ─────────────────────────────────────────────────────────────────────

# Build debug binary
build:
    cargo build

# Build release binary
release:
    cargo build --release

# Run tests
test:
    cargo test

# Check types without producing a binary
check:
    cargo check

# ── Run ───────────────────────────────────────────────────────────────────────

# Run distq with arbitrary arguments  (e.g.: just run curl)
run *ARGS:
    cargo run -- {{ARGS}}

# Quick smoke test: search 'curl' across the linux profile
smoke:
    cargo run -- --profile linux curl

# ── Docker images ─────────────────────────────────────────────────────────────

# Build all distq/* docker images
docker-build:
    cargo run -- docker build

# Build only images that are not yet present
docker-build-missing:
    cargo run -- docker build --missing

# List distq/* images currently on this host
docker-list:
    cargo run -- docker list

# Build a single distro image  (e.g.: just docker-one arch)
docker-one DISTRO:
    docker build --no-cache \
        -f dockerfiles/Dockerfile.{{DISTRO}} \
        -t distq/{{DISTRO}} \
        dockerfiles/

# Smoke-test a single distro image directly  (e.g.: just docker-test arch curl)
docker-test DISTRO PKG:
    docker run --rm --network=none distq/{{DISTRO}} {{PKG}}

# ── Development ───────────────────────────────────────────────────────────────

# Regenerate ~/.config/distq/config.toml with defaults
init-config:
    cargo run -- --init-config

# Show current config path and contents
show-config:
    @echo "=== $(distq --init-config 2>&1 | head -1 | grep -o '[^ ]*config.toml')" || true
    @cat ~/.config/distq/config.toml 2>/dev/null || echo "(no config file)"

# Clippy lint
lint:
    cargo clippy -- -D warnings

# Format source
fmt:
    cargo fmt

# ── Release packaging ─────────────────────────────────────────────────────────

# Build release and package into dist/distq-<arch>.tar.gz
package:
    cargo build --release
    @mkdir -p dist
    tar -czf dist/distq-x86_64-unknown-linux-gnu.tar.gz \
        -C target/release distq \
        -C ../../ dockerfiles
    @echo "Created dist/distq-x86_64-unknown-linux-gnu.tar.gz"

# Clean build artifacts
clean:
    cargo clean
    rm -rf dist/
