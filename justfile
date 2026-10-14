# andon — command runner

set shell := ["bash", "-euo", "pipefail", "-c"]

[private]
default:
    @just --list

# All checks: format, lint, tests, release-script tests. The land gate.
[group('check')]
check: fmt-check lint test test-release

# cargo fmt --check
[group('check')]
fmt-check:
    cargo fmt --check

# clippy with warnings as errors; bash -n and shellcheck over the scripts
[group('check')]
lint:
    cargo clippy --all-targets --locked --quiet -- -D warnings
    bash -n scripts/*.sh scripts/jj-identity
    shellcheck -S warning scripts/*.sh scripts/jj-identity

# Unit and integration tests
[group('check')]
test:
    cargo test --locked --quiet

# Live acceptance: andon's adapters against real projects' stores, from the
# suites in ~/.config/andon/live.toml (or $ANDON_LIVE). Not part of `check`:
# another project's stores change under andon.
[group('check')]
live:
    cargo test --locked --test live -- --ignored --nocapture

# Unit tests for scripts/release.py
[group('check')]
test-release:
    python3 scripts/test_release.py

# Publish the described jj change: run `just check` on exactly it, then move main
# (and push it, once a forge remote named origin exists)
[group('vcs')]
land *args:
    scripts/jj-land.sh {{args}}

# Add a jj workspace for one more agent (run in ~/code/andon): ../<name>, with bd and Claude memory linked
[group('vcs')]
workspace-add name:
    scripts/workspace-add.sh {{quote(name)}}

# Build the Nix package and print its version
[group('build')]
build:
    out="$(nix build --no-link --print-out-paths .)" && "$out/bin/andon" --version

# Set the version in Cargo.toml and scaffold the CHANGELOG entry
[group('release')]
[arg('version', pattern='[0-9]+\.[0-9]+\.[0-9]+', help='Semver release, e.g. 0.3.1')]
release-bump version:
    python3 scripts/release.py bump {{quote(version)}}

# Every advertised Nix package output is in the public Cachix cache
[group('release')]
cache-verify:
    python3 scripts/release.py cache-verify

# Release readiness: versions agree, changelog filled, clean tree, checks, Nix build
[group('release')]
release-verify:
    python3 scripts/release.py verify

# Verify the cache, tag vX.Y.Z, push it, move origin/release; GitHub publishes the release
[group('release')]
[arg('version', pattern='[0-9]+\.[0-9]+\.[0-9]+', help='Semver release, e.g. 0.3.1')]
[confirm("This will verify cached Nix outputs, tag, push the tag, force-update origin/release and publish a GitHub release. Continue?")]
release-tag version:
    python3 scripts/release.py tag {{quote(version)}}
