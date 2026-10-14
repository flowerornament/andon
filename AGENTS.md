# andon

The signal board for many agents on one repository, derived from artifacts. `README.md` is the product document; `.design/` holds the founding design corpus (dated `YYYY-MM-DD-kebab-title.md`; amendments append, never edit).

## Invariants (violations are bugs)

- **Read-only.** andon writes nothing into any tracker, messenger, runner or repository. Its only effects are stdout and a non-zero exit from a `check`.
- **Derived, never maintained.** Every surface is a fold over the event ledger. No surface depends on anyone updating a status by hand. If a fact cannot be derived from an artifact, it is not on the board.
- **Every refusal names the fix**, in one line, on stdout.
- **stdout is an API.** Hooks inject `andon hook` stdout into agent context; `--json` is machine-read; decoration goes to stderr behind `is_terminal`.
- **No wall-clock reads in a fold path.** Time enters only at capture and render; `asof` must reproduce a past board byte-for-byte.
- **Facts, not judgment.** The board shows what artifacts record. Agent-asserted memory (peat `obs` and the like) is never an input.
- **Conventions live in adapter config, not in the fold.** A trailer name, a subject pattern or a bot account is configuration.
- **A check must be able to fail.** Every refusal ships with a test that makes it fire.

## Build

Rust, one binary, edition 2024 on the toolchain pinned in `rust-toolchain.toml`. `just check` (fmt, clippy `-D warnings`, cargo tests, release-script tests, shellcheck) is the land gate; CI runs the same recipe on Linux and macOS. Dependencies stay on their latest releases: add with `cargo add`, and bump them in their own change.

## Version control: jj, one workspace per agent

```text
~/code/andon      colocated checkout (.git + .jj): bd home, the git object store.
                  With several agents, no agent edits code here.
~/code/andon-1a   jj workspace: coordinator (reviews, gives GO, releases)
~/code/andon-1b   jj workspace: implementer
```

Each writing agent gets its own workspace, so it never snapshots another agent's half-finished edits. hail names a seat after the jj root, so each workspace is also a hail address.

- **Work:** `jj new main`, edit, then `jj describe -m "area: subject (andon-xxx)"`. To pick up others' work, `jj rebase -d main`.
- **Publish:** `just land`. It runs `just check` on your change, with any described commits beneath it, and only then moves `main` and pushes it. It refuses undescribed commits in the range, a stale parent, and any edit made while the check ran. With no `origin` remote it moves `main` and pushes nothing. Never `jj git push` by hand: that skips the gate.
- **Trailers:** a SessionStart hook (`scripts/jj-identity`) adds `Jj-Workspace:`, `Agent:` and `Session:` to every description. Don't type or strip them; andon's own commit → session join reads them.
- **Git:** run it only in `~/code/andon`, and only to read. A workspace has no `.git`, and `~/.git` exists, so git in a workspace silently answers about your home directory.
- **Processes:** stop a process only by its exact pid. Never `pkill -f`, `killall` or any pattern match: this machine is shared by many agents' seats.
- **Stale workspace:** when another agent rewrites a commit under your `@`, run `jj workspace update-stale`. Never `jj edit` a change that is an ancestor of another workspace's `@`.
- **Recovery:** start with `jj op log`, `jj undo` or `jj op restore`, before any destructive file operation. The `jj-ops` skill has the full model.
- **New workspace:** `just workspace-add andon-<pair><letter>` in `~/code/andon`. It also points bd and Claude's memory at the main checkout. `jj workspace list` is the roster.

## Done means landed

1. `jj describe -m "area: subject (andon-xxx)"`, then `just land`.
2. `bd close <id>`, then `bd dolt push`.

## Goals (bd)

andon's own tracker is bd, and it holds goals only (`.design/2026-10-13-andon-founding.md` §3). This section overrides bd's generic guidance, including what `bd prime` prints.

- **The unit is a goal:** why, a done-when that can fail, an appetite. The how is yours and never becomes an item.
- **Findings go on the goal** as a `bd comments add` comment; its owner keeps or drops them when it closes. Nothing is filed "for follow-up", and no item is created for a plan step.
- **Every landing cites a goal** in its subject: `area: subject (andon-xxx)`.
- **Memory** is not kept in bd (`bd remember`) or in ad hoc files; durable facts belong in the code, the living documents or a dated design document.

Commands: `bd ready`, `bd show <id>`, `bd update <id> --claim`, `bd close <id>`. Goals live in a local Dolt database; sync with `bd dolt push` / `bd dolt pull` to the file remote, never through git. `.beads/config.yaml` is tracked config only; keep machine paths out of it.

## Release

Cargo.toml's version is the one version source (`andon --version`, `package.nix`, the release tooling). From `~/code/andon`, never a workspace:

1. `just release-bump X.Y.Z`, fill the CHANGELOG entry, describe and `just land`.
2. Wait for the Nix Cache workflow on that commit (`just cache-verify`).
3. `just release-verify`, then `just release-tag X.Y.Z`: it tags, pushes the tag and moves `origin/release`. `.github/workflows/release.yml` publishes the GitHub release with the CHANGELOG section and binaries for four targets.

`package.nix` builds from an allowlist (`Cargo.toml`, `Cargo.lock`, `src`, `tests`); add a path there when the build or its tests start reading it. `.design/` never enters the package.

## How the product builds itself

andon's own development is supervised through andon as soon as `board` runs: the goal schema applies to this repo's own tracker, and every landing cites a goal. Until then, goals live in bd, seeded from the milestones in `.design/`.
