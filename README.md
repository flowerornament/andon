# andon

The signal board for many agents on one repository, derived from the artifacts they already write.

andon reads what a team of agents already leaves behind (commits, landings, review verdicts, test runs, tracker claims, session transcripts, messages) and shows, per seat: the goal it holds, the budget it has spent, what it landed, how its runs went, its last verdict, and what needs acting on. Nobody updates a status. andon writes nothing anywhere; it prints, and its checks exit non-zero with the fix named.

```
$ andon
board as of 2026-10-26 09:00 PDT · counts over the last 24h

seat     goal         budget         lands   landed  runs  pass  unlanded  verdict         comments
proj-1a  proj-k3v     1.5/4 agent-h      2   1h ago     4  100%         0  GO 1h ago              0
proj-1b  proj-q8d     4.5/3 agent-h      1  21h ago     9   56%         9  RETURN 20h ago         0
proj-2a  proj-w2h     1.0/2 agent-h      3  40m ago     3  100%         0  GO 30m ago             0
proj-2b  proj-m7p +1  6.0/4 agent-h      0   6d ago     0     —         0  GO 6d ago              0

flags
  proj-1b  over budget on proj-q8d
  proj-2b  2 goals in progress, WIP 1 · over budget on proj-m7p · no landing in 5.2d

1 quiet seat (--all shows it): proj-3a
```

A `?` is a value no source covers, never a 0; the reason prints beneath the table. `andon board --help` defines every column and flag.

## Quickstart

1. **Install** (Nix, from the flake; binaries for macOS and Linux are on each release):

   ```sh
   nix run github:flowerornament/andon -- --version
   ```

2. **Configure.** In the repository, write a starter config and edit it:

   ```sh
   andon init > andon.toml
   ```

   Name each seat (one per agent or workspace) under `[seats]`, and point the adapters at the tools you use. Identity comes first: every fact joins to a seat by the name its tool recorded, so each agent must act under its seat's name. With bd, set `BEADS_ACTOR=<seat>` in the seat's environment; a claim made as your git user belongs to no seat, and the board says so.

3. **Look.**

   ```sh
   andon                         # the board, now
   andon asof yesterday          # as it stood at the end of yesterday
   andon scorecard 7d            # the four gauges over the last week
   ```

4. **Wire the refusals** into the choke points the project already has: `andon check commit @` in the land verb, `andon check goal <id>` when a goal is filed, `andon check message <kind>` in the messenger's send.

5. **Put the board in each agent's turn.** Add `andon hook` as a Claude Code `UserPromptSubmit` (or `SessionStart`) hook, or a Codex hook. Each turn starts with one line: the seat's row, and what changed since its last turn. `echo '<payload>' | andon hook --check` checks the wiring.

## Commands

| Command | What it does |
|---|---|
| `andon` / `andon board [--seat S] [--all] [--json]` | One row per seat now. Quiet seats (no goal, nothing in the window) fold into one line. |
| `andon asof WHEN` | The board as it stood at an instant, replayed from the ledger. A date means its end. |
| `andon scorecard [WINDOW]` | Per seat over a window (default today): landings, runs, verdicts, messages, tracker items, active hours; then flow medians, the tracker's branching, the hub and how well the joins held. |
| `andon check commit REV` | The commit cites a goal its seat has claimed, within WIP. |
| `andon check goal ID` | The goal has its sections, a done-when that can fail, a finite budget, and room in its pool. |
| `andon check message KIND` | A decision names the goal it serves. |
| `andon hook` | The seat's row for a harness hook. Never fails a session. |
| `andon goals [--seat S] [--all] [--json]` | The goals: open ones by default, who holds each (a name that is no seat is marked `*`), claimed or not, budget spent of allowed, and how long since the claim. |
| `andon seats [--json]` | The roster: each seat's aliases and prefixes, where it works, its live session and what joined to it in the window; then every name the sources recorded that joins no seat, with the fix. |
| `andon doctor` | What each configured source read: its events, the span it covers, its gaps and its time, or its whole error; then the hook's wiring. Exits 1, naming the fix, when a source cannot be read. |
| `andon ledger` | Every event as JSONL, to keep; `--ledger FILE` reads it back instead of the sources. |
| `andon init` | An `andon.toml` for this repository, from what it finds: its name, jj or git and the trunk, bd's issue prefix, jj workspaces as seats, Claude Code's session files and hail. |

**Times.** Text prints times in the local zone, with the zone named; `TZ=UTC andon …` prints UTC, and `--json` is always UTC. A time is a date (`2026-10-24`), a local time (`2026-10-24T09:30`), an exact one (`…Z` or an offset), `now`, `today`, `yesterday`, `-6h` or `-2d`. A window is a day, `FROM..TO`, `FROM..`, or the last `6h` or `2d`.

**Output.** On a terminal the board fits the width and is colored (`--color auto|always|never`; `NO_COLOR` is honored). Piped, it is plain text that is never cut. `--json` and the hook's line follow a versioned contract (`.design/2026-10-19-output-contract.md`). While sources are read, a terminal's stderr shows one progress line; `--timings` prints each source's read time and event count to stderr after the output.

**Cache.** andon writes into no repository or tracker. To read in seconds it keeps transcript parses, jj's operation log and bd's event rows under `~/.cache/andon` (`$XDG_CACHE_HOME/andon`, or `$ANDON_CACHE`), each checked against its source before use: deleting it changes no output, and `ANDON_CACHE=off` keeps nothing.

**Exit codes.** 0 rendered or passed; 1 a check refused, and stdout names the fix in one line; 2 andon could not decide, and the line says what to repair.

## Adapters

All read-only. Configure the ones the project uses:

| Adapter | Reads |
|---|---|
| `jj`, `git` | landings on the trunk, commit trailers, each jj workspace's unlanded files |
| `bd`, `github` | goals: creates, claims, closes, comments and review verdicts; GitHub Actions runs |
| `ferry` | runner receipts |
| `sessions` | Claude Code and Codex transcripts: activity, spawns, edits |
| `presence` | Claude Code's session files: who is running now, busy or idle |
| `hail` | messages between seats |
| `external` | anything else: a project's own program or files that print andon events as JSONL |

Every adapter records what it could see. A gauge whose span its sources do not cover reads `?` with the reason (`.design/2026-10-14-coverage.md`). A project's conventions (a trailer name, a subject pattern, a bot account) are configuration, never code; andon's code names no project (`tests/boundary.rs`). The external adapter contract is `.design/2026-10-14-project-boundary.md`.

## Configuration reference

`andon.toml` is read from the current directory or its nearest ancestor (`--config FILE` names another). Every key, at its default; a key marked *example* has no default and shows a typical value. Unknown keys are refused.

```toml
[project]
name = ""                                   # the project's name
goal_pattern = '\(([a-z][a-z0-9]*-[a-z0-9]{3,8}(?:\.[0-9]+)*)\)'  # a goal id; its group is the id
goal_in = "subject"                         # where a commit cites its goal: subject | message
known_goals_only = true                     # keep only ids the tracker has
land_recipe = "land"                        # the gate recipe that is the land verb

[seats.proj-1a]                             # one table per seat (example)
aliases = ["proj-1a"]                       # exact labels: trailers, tracker actors, message labels
prefixes = ["proj-1a-"]                     # labels starting with these are this seat's
cwd = ["~/code/proj-1a"]                    # working directories whose sessions are this seat's
wip = 1                                     # overrides goal.wip_per_seat (example)

[pools.core]                                # a pool of goals (example)
cap = 4                                     # open goals the pool may hold
seats = ["proj-1a"]

[goal]                                      # declare it, even empty, for the goal schema
sections = ["Why", "Done when", "Budget"]   # sections a goal's description must carry
done_when = "Done when"                     # which section must name a check that can fail
budget = "Budget"                           # which section must be finite
wip_per_seat = 1                            # goals in progress per seat
require_pool = false                        # require a Pool section naming a pool
types = ["feature", "bug", "task"]          # item types that count toward WIP
checkable = [                               # words a done-when should use to be checkable
  "pass", "fail", "refus", "exit", "print", "return", "reproduc", "assert", "equal",
  "match", "land", "merge", "close", "exist", "show", "fire", "contain", "succeed",
  "admit", "accept", "reject", "exceed", "report", "warning", "error", "zero", "under",
  "below", "above", "within", "at most", "at least", "no more than", "fewer than",
  "more than", "no seat", "no warnings", "every",
]

[goal.fields]                               # sections kept in tracker fields (example)
"Done when" = "acceptance_criteria"
"Budget" = { field = "estimate", unit = "agent-hours" }   # or "estimated_minutes"

[messages]
decision_kinds = ["ask", "go", "nogo", "ruling", "block", "stop"]  # kinds that must name a goal
kinds = ["ask", "go", "nogo", "ruling", "block", "stop", "fyi", "done", "announce", "hold", "release"]
hubs = []                                   # seats whose inbound load is the hub gauge; [] = busiest

[board]
window_hours = 24.0                         # the window the board's counts cover
no_land_days = 1.0                          # flag a held goal with no landing in this many days
runs_without_land = 20                      # flag this many runs since the last landing
idle_minutes = 60.0                         # flag a live session idle this long holding a goal
budget_unit = "agent-hours"                 # how agent-hours are counted: agent-hours | wall-clock

[adapters.jj]
repo = "."                                  # any directory in the repository
trunk = "main"
since = "2026-10-01"                        # read history from here (example; default: all)

[adapters.jj.trailers]                      # trailer names, by role
session = "Session"
workspace = "Jj-Workspace"
seat = "Seat"                               # names the committing seat (example; default: none)
critic = "Critic"                           # cites a verdict (example; default: none)
critic_id_word = 1                          # which word of the citation is the verdict id
keep = []                                   # other trailers kept on landings

[adapters.git]                              # instead of, or beside, jj
repo = "."
trunk = "main"
branch_seats = { "proj-1a/" = "proj-1a" }   # branch prefix → seat (example)
pr_pattern = '\(#(\d+)\)\s*$|^Merge pull request #(\d+)'
since = "2026-10-01"                        # (example)

[adapters.bd]
dir = "."                                   # where bd resolves the project's database
events = true                               # read claims and closes as history from bd's events
events_zone = "America/Los_Angeles"         # zone of rows stored as local time (example; default: this machine's)
events_utc_from = "2026-10-15T00:00:00"     # first row stored as true UTC (example; default: calibrated)
since = "2026-10-01"                        # (example)

[adapters.bd.verdicts]                      # review verdicts written as bd comments (example)
prefix = "VERDICT "                         # then a JSON object
outcome = "outcome"                         # dotted paths into that object
commit = "commit"
change = "changes"
session = "critic.session"

[adapters.github]
repo = "owner/name"                         # (example; required)
goal_label = "goal"                         # an issue with this label is a goal (example; default: every issue)
accounts = { "proj-bot" = "proj-1a" }       # login → seat (example)
branch_seats = { "proj-1a/" = "proj-1a" }   # branch prefix → seat, for Actions runs (example)
limit = 200                                 # issues, pull requests and runs read
runs = true                                 # read Actions runs
snapshot = "tests/fixtures/gh"              # read JSON here instead of calling gh (example)
since = "2026-10-01"                        # (example)

[adapters.ferry]
receipts = ["~/code/proj*/.ferry/receipts"] # receipt directories; * within one component

[adapters.sessions]
claude = ["~/.claude/projects/*-code-proj*"]  # Claude Code project directories
codex = ["~/.codex/sessions"]                 # Codex session directories
cwd = ["~/code/proj*"]                        # keep sessions whose working directory matches
gap_minutes = 10                              # a gap this long ends an activity span
since = "2026-10-01"                          # (example)

[adapters.presence]
claude = ["~/.claude/sessions"]             # Claude Code's per-session files

[adapters.hail]
state = "~/.local/state/hail"
labels = ["proj*"]                          # keep messages to or from these
since = "2026-10-01"                        # (example)

[[adapters.external]]                       # a project's own adapter (example)
name = "proj-review"                        # marks every event it emits
command = ["scripts/andon-review"]          # prints events as JSONL; or:
files = []                                  # files of events
dir = "."                                   # where the command runs
contract = 1
```

## Why

Agents coordinating through messages fail in a known way: channels grow as n², a hub agent saturates, status drowns decisions, and the tracker fills with tasks nobody claims. The artifacts tell the whole story without any of it. On one project, two days of six seats reconstructed from artifacts alone joined 117 of 117 landings to their sessions and 131 of 131 reviews to their verdicts, while 1,424 messages carried mostly status and 13% named the work they were about.

Four laws of flow govern any such line, and each has a gauge here:

| Law | Statement | Gauge |
|---|---|---|
| Little | L = λ · W | work in progress per seat |
| Branching | the backlog is bounded iff m < 1 | items spawned per item closed |
| Rework | expected tries = 1 / p | runs per landing; whether failures keep their reason |
| Single server | wait ∝ ρ / (1 − ρ) | messages to any hub |

Agents, unlike people, pay nothing to create an item, a message or a run, so the limits human organizations get for free from attention and friction must be built in, and enforced by tools, because written norms hold poorly. On a factory line the andon is the board that shows every station's state from the line itself, and the cord a worker pulls when a station is stuck.

```
 adapters (read-only)          fold                      surfaces
 ┌──────────────────────┐    ┌──────────────────┐    ┌───────────────────────┐
 │ vcs       jj | git   │    │ one event ledger │    │ board     row per seat│
 │ tracker   bd | gh    │───►│ {t, seat, kind,  │───►│ asof      any instant │
 │ runner    ferry | gh │    │  ids, outcome,   │    │ scorecard any window  │
 │ sessions  transcripts│    │  attrs}          │    │ hook      in the turn │
 │ presence  live files │    │ joins by id      │    └───────────────────────┘
 │ messages  hail       │    └──────────────────┘    ┌───────────────────────┐
 │ external  any JSONL  │                            │ check commit · goal · │
 └──────────────────────┘                            │ message: exit 1, fix  │
                                                     └───────────────────────┘
```

Three contracts make it project-independent: **the goal schema** (a goal has a why, a done-when that can fail and a budget; the how is the agent's and never becomes an item), **the event ledger** (every adapter emits one event type, each attribute declared once in `src/event.rs`, joined by id), and **the refusals** (called from the project's own choke points; andon does not own them). `.design/` holds the founding design, the study behind it and every decision since.

## Development

`AGENTS.md` is the working agreement: invariants, the `just check` gate, the jj workflow (one workspace per agent, publish with `just land`), goals in bd, and the release steps. `CHANGELOG.md` records each release.
