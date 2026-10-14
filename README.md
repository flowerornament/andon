# andon

The signal board for many agents on one repository, derived from the artifacts they already write.

On a factory line the andon is the board that shows every station's state from the line itself, and the cord a worker pulls when a station is stuck. This tool is that board for agent teams: it reads commits, merges, review verdicts, test runs, issue claims and session transcripts, folds them into one event ledger, and shows each seat's goal, effort, landings, runs and verdicts. It never stores work, never carries messages, never runs anything. It reads, derives, and refuses at the choke points a project already has.

## Why

Agents coordinating through messages fail in a known way: channels grow as n², a hub agent saturates, status drowns decisions, and the tracker fills with tasks nobody claims. The artifacts tell the whole story without any of it. On one project, two days of six seats reconstructed from artifacts alone joined 117 of 117 landings to their sessions and 131 of 131 reviews to their verdicts, while 1,424 messages carried mostly status and 13% named the work they were about.

Four laws of flow govern any such line, and each has a gauge here:

| Law | Statement | Gauge |
|---|---|---|
| Little | L = λ · W | work in progress per seat |
| Branching | the backlog is bounded iff m < 1 | items spawned per item closed |
| Rework | expected tries = 1 / p | runs per landing; whether failures keep their reason |
| Single server | wait ∝ ρ / (1 − ρ) | messages to any hub |

Agents, unlike people, pay nothing to create an item, a message or a run, so the limits human organizations get for free from attention and friction must be built in, and enforced by tools, because written norms hold poorly.

## What it does

```
 adapters (read-only)            fold                      surfaces
 ┌───────────────────┐    ┌──────────────────┐    ┌────────────────────────┐
 │ vcs     jj | git  │    │ one event ledger │    │ board    row per seat  │
 │ tracker bd | gh   │───►│ Event{t, seat,   │───►│ scorecard per day      │
 │ review  verdicts  │    │  kind, ids,      │    │ asof     any past day  │
 │ runner  ferry|ci  │    │  outcome}        │    │ hook     row into the  │
 │ sessions peat|–   │    │ joins by ids     │    │          agent's turn  │
 │ messenger hail|–  │    └──────────────────┘    └────────────────────────┘
 └───────────────────┘                                      │
                                   refusals (exit ≠ 0, fix named)
                                   ┌──────────────────────────────┐
                                   │ check commit  cites a goal   │
                                   │               claimed by seat│
                                   │ check goal    why·done-when· │
                                   │               budget·pool  │
                                   │ check message names a goal   │
                                   └──────────────────────────────┘
```

Three contracts make it project-independent:

1. **The goal schema.** The unit of work is a goal: why, a falsifiable done-when, a budget, a pool, an owner. The how belongs to the agent and never becomes a tracked item. The tracker stores goals; andon validates them.
2. **The event ledger.** Every adapter emits one type: `{t, seat, kind, subject, ids{commit, change, goal, verdict, receipt, session}, outcome}`. Joins are by id, so a project's conventions (trailers, subject prefixes, branch names, bot accounts) live in adapter config, not in the fold.
3. **The refusals.** Called from the project's own choke points (its land or merge verb, its goal creation, its messenger's send), each exits non-zero and names the fix. andon does not own the choke point.

## Surfaces

- `andon board [--seat S]` — goal, budget spent of allowed, landings, runs and pass rate, last verdict, findings on the lead goal since its claim, flags (over budget, more goals than WIP allows, no landing in N days, last land refused; a hub in `[messages] hubs` gets no landing flags). Only `[goal] types` (default feature, bug, task) count toward WIP. A goal claimed by an identity that is no seat is named beneath the table.
- `andon scorecard <day>` — the four gauges per seat.
- `andon asof <date>` — the board as it stood, replayed from the ledger.
- `andon hook` — reads the harness hook JSON on stdin, prints the seat's row. Claude Code and Codex.
- `andon check commit <rev>` · `check goal <id>` · `check message <kind> [--goal]` — the refusals.

## Status

All three milestones of the founding design are built. `board`, `scorecard`, `asof`, `hook`, `ledger` and the three `check` refusals run over adapters for jj, git, bd, GitHub (issues, reviews, Actions), Ferry, Claude Code and Codex transcripts, and hail. Every headline number of the study is reproduced from its recorded events (verified privately, and by an independent acceptance test against the originating project's live stores); this repository's `just land` refuses a commit that cites no goal its seat holds; `tests/github.rs` reads a GitHub-native repository from configuration alone. `.design/` holds the spec and what each milestone decided. The study that motivated it (the loop trace of one project over 2026-10-11..12, six implementing seats over two days) is kept privately.

Three milestones, from the spec:

1. **Instrument.** Adapters for jj, bd, Ferry and sessions; `board`, `scorecard`, `asof`, `hook`, `check commit`. Done when `andon scorecard` for 2026-10-11..12 reproduces the study's headline numbers.
2. **Enforce.** `check goal` and `check message`, wired into the project's land verb and messenger. Done when each check fires in a test and the project's land refuses an uncited commit.
3. **Generalize.** git and GitHub adapters. Done when a GitHub-native repository shows a board with no code change in andon.

## Use

**First, identity.** andon joins every fact to a seat by the names the
tools record, so each agent must act under its own seat's name. A tracker
claim made as a shared default user (bd assigns your git user unless
`BEADS_ACTOR` is set) belongs to no seat, and andon says so. Set each
seat's identity in its environment, for example `BEADS_ACTOR=my-project-1a`,
and list the labels a seat is known by under `[seats]`.

```sh
andon init > andon.toml          # a starter config: seats, goal rules, adapters
andon board                      # one row per seat, now
andon asof 2026-10-12T12:00Z     # the board as it stood
andon scorecard 2026-10-11..2026-10-12
andon ledger > ledger.jsonl      # keep the history; --ledger ledger.jsonl reads it back
andon check commit @             # exit 1 and one line naming the fix, or exit 0
```

A GitHub project needs only configuration:

```toml
[project]
goal_pattern = '(?:[Cc]loses|[Ff]ixes) (#\d+)'   # the capture group is the goal id
goal_in = "message"
[seats.alice]
aliases = ["alice-gh", "Alice Smith"]           # login and git author name
[adapters.git]
repo = "."
trunk = "main"
[adapters.github]
repo = "owner/name"
```

A goal section kept in a tracker field rather than the description is mapped in config, for example bd's acceptance field:

```toml
[goal.fields]
"Done when" = "acceptance_criteria"
```

A scorecard window that ends after now counts up to now and says so. Exit codes: 0 rendered or passed; 1 a check refused; 2 andon could not decide (the line says what to repair). `andon hook` reads a Claude Code or Codex hook payload on stdin and never fails the session.

## Adapters

Core adapters read general tools: jj, git, bd, GitHub, Ferry receipts, hail, and Claude Code and Codex transcripts. Anything particular to one project (its evidence store, its desk layout, its review records) reaches andon through an external adapter the project owns: a program that prints andon events as JSONL, declared by name in the project's `andon.toml`.

```toml
[[adapters.external]]
name = "proj-review"
command = ["scripts/andon-review"]
```

Every event an external adapter emits is marked with its name. Every adapter also records what it could see: `coverage` events name the kinds and span it read, and any source it could not read. A gauge whose window is not covered reads `?` with the reason, never 0 (`.design/2026-10-14-coverage.md`). A tracker export is current state: it vouches for the goals held only at the instant it was read, so `asof` an earlier instant shows them as `?` unless a kept ledger holds a capture from then. andon's code knows no project; `tests/boundary.rs` holds that line. The contract (event schema, protocol, versioning) is `.design/2026-10-14-project-boundary.md`.

## Install

With Nix, from the flake (packages for aarch64-darwin, aarch64-linux and x86_64-linux, substituted from the `flowerornament` Cachix cache):

```sh
nix run github:flowerornament/andon -- --version
```

Tagged releases also attach binaries for macOS and Linux on arm64 and x86_64. To build from source: `cargo build --release`, on the Rust toolchain in `rust-toolchain.toml`.

## Development

`AGENTS.md` is the working agreement: invariants, the `just check` gate, the jj workflow (one workspace per agent, publish with `just land`), goals in bd, and the release steps. `CHANGELOG.md` records each release.
