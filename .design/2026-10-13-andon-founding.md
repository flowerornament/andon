---
title: andon, founding design
date: 2026-10-13
status: living
---

# andon: founding design

## 1. What it is for

A project with several agents working one repository needs exactly five coordinations:

1. ownership: the same work is not done twice;
2. direction: the work serves a stated why;
3. safety: nothing lands unreviewed or ungated;
4. observability: stuck and done are visible without asking;
5. conflict: what no seat can decide alone reaches whoever can.

Everything else is overhead. A study of one project's agent loop (six implementing seats over two days) found (3) working (a change reached master in a median 33 minutes, every publish reviewed) and (4) and (5) failing: status that should have been observable was sent as messages (1,424 in two days, 13% naming the work), and a coordinator became a queue (400 obligations swept unanswered). The diagnosis in one line: coordination was built out of messages before observability was built out of artifacts. Messages need a reader and scale as n²; artifacts scale as n and need no reader.

andon supplies (4) by derivation and supports (1), (2) and (5) by refusing at choke points. It does not replace a tracker, a messenger, a runner or a memory.

## 2. The laws and their gauges

| Law | Statement | Study value | Gauge |
|---|---|---|---|
| Little | L = λ·W | pipeline ≈ 2 in flight; backlog 969 open, wait ≈ 18 d | WIP per seat; open items per pool |
| Branching | bounded ⟺ m < 1 (ρ(A) < 1 by kind) | m ≈ 1.4 all time, 1.55 last week | items spawned per item closed, per seat per day |
| Rework | E[tries] = 1/p | one seat: 383 candidates, 0 landed; 364 of 434 failures kept no reason | runs per landing; failures with a kept reason |
| Single server | wait ∝ ρ/(1−ρ), ρ = nλ/μ | median reply 989 min while the hub was offline | messages to any hub per day; asks without a goal |

Agents pay nothing to create an item, a message or a run. The friction human organizations get from attention and social cost must be configured and enforced.

## 3. The goal schema

The unit of work is a **goal**, not a task.

| Field | Meaning | Check |
|---|---|---|
| why | the obligation or outcome it serves | present |
| done-when | a check that can fail | present; not "improve" |
| appetite | the budget: wall-clock, agent effort, or runs | present, finite |
| pool | the area whose seats may claim it | names a configured pool |
| owner | the seat that claimed it | set by claim, never by hand |

Three things with different lifetimes must not share one type: goals (live until done-when holds), plans (the how; live with the agent's session), findings (noticed; live until the goal closes, then kept or dropped by its owner). Giving them one type and one lifecycle is what makes m > 1. andon validates goals; plans and findings are never ledger items.

On appetite exhaustion the owner lands what holds, narrows the goal within its why, or parks it with a finding. andon flags; the project's rules decide.

## 4. The event ledger

One type, emitted by every adapter:

```
Event {
  t:        timestamp (UTC)
  t_end:    timestamp | null
  seat:     string | null          # resolved by the seat adapter
  kind:     land | freeze | commit | verdict | gate_run | run |
            measurement | message | claim | close | session_active |
            spawn | other:<x>
  subject:  goal id | null
  ids:      { commit, change, tree, goal, verdict, receipt, gate_run,
              session, critic_session, message }
  outcome:  pass | fail | go | return | refused | stopped | unknown | null
  source:   path or command that produced it
}
```

Joins hold by id. The study measured which joins hold on one project:

| Join | Rate | Key |
|---|---|---|
| commit → session | 117/117 | `Session:` trailer |
| review trailer → verdict | 131/131 | change id (commit ids never match: land rewrites) |
| landing → gate run | 43/55 since recording began | tree |
| run receipt → commit | exact | receipt.commit; the measured commit may be in args |
| run receipt → goal | 59% | via the commit's subject |
| comment → seat | 19% solid | one shared author: configure an actor per seat |

The ledger is append-only and versioned. `asof` replays a prefix.

## 5. Adapters

Each adapter reads one source and emits events. Conventions are configuration.

| Adapter | Reads | Emits | Config |
|---|---|---|---|
| vcs: jj | op log (publishes), commits and trailers, workspaces | land, commit, freeze, desk create/remove | trailer names; subject pattern for the goal id; desk → seat map |
| vcs: git | reflog of the trunk, merges, trailers, branches | land, commit | branch prefix → seat; trailer names |
| tracker: bd | issues, comments, events | claim, close, verdict (parsed JSON comments), finding | actor → seat; goal sections |
| tracker: github | issues, PRs, reviews | claim, close, verdict | bot account → seat; issue template |
| runner: ferry | receipts (caller side and host store) | run, measurement | receipt dirs |
| runner: ci | workflow runs | run | |
| sessions: peat | ledger (tool calls, sessions, commits) | session_active, spawn | session cwd → seat |
| messenger: hail | state dir | message | label → seat |

Seat identity is the one join every adapter needs. A project declares how a seat is known in each source (trailer, actor, cwd, label, account).

## 6. Surfaces

- `board [--seat S] [--json]`: per seat: goal, appetite used, landings (count, last), runs (count, pass rate), last verdict, open findings, flags.
- `scorecard <day> [--json]`: the four gauges per seat, plus items created and closed.
- `asof <date>`: the board at that date, deterministic.
- `hook`: reads the harness hook JSON on stdin; on prompt submit prints the seat's row and what changed since the seat's last turn (claimed, GO, RETURN, land refused, appetite spent); on session start prints the row. Silent when nothing changed.

## 7. Refusals

Exit non-zero, one line naming the fix. Called by the project at its own choke points.

| Check | Refuses when | Fix named |
|---|---|---|
| `check commit <rev>` | the commit cites no goal; the goal is not claimed by this seat; the seat has more goals in progress than its WIP | cite the goal; `claim`; land or park one |
| `check goal <id>` | a required section is missing or the done-when cannot fail; the pool is at its cap | add the section; land or defer one |
| `check message <kind>` | a decision kind carries no goal | name the goal |

Every check ships with a test that makes it fire.

## 8. Configuration

```toml
[seats]
"proj-1a" = { jj_trailer = "Seat", bd_actor = "proj-1a", hail_label = "proj-1a", cwd = "~/code/proj-1a" }

[pools]
compiler = { cap = 5, seats = ["proj-1a", "proj-4a"] }

[goal]
sections = ["Why", "Done when", "Appetite"]
appetite_unit = "agent-hours"      # wall-clock | agent-hours | runs
wip_per_seat = 1
review_queue_cap = 3

[adapters]
vcs = "jj"; tracker = "bd"; runner = "ferry"; sessions = "peat"; messenger = "hail"
```

## 9. What andon is not

Not a tracker (bd, GitHub Issues), not a messenger (hail), not a runner (Ferry, CI), not memory (peat). It reads them. Its only writes are stdout and exit codes.

## 10. Milestones

1. **Instrument the originating project.** Adapters jj, bd, ferry, peat; `board`, `scorecard`, `asof`, `hook`; `check commit`. Port the study's Python scripts to Rust. Done when `scorecard` for 2026-10-11..12 reproduces the study's numbers from its recorded events.
2. **Enforce.** `check goal`, `check message`; wire into the project's land verb and messenger. Done when each check fires in a test and the project's land refuses an uncited commit.
3. **Generalize.** git and GitHub adapters; a second project runs `board`. Done when a GitHub-native repository shows a board with no code change in andon.

## 11. Open questions

- Appetite unit by default: wall-clock is derivable everywhere; agent effort needs a sessions adapter.
- Whether the ledger should be peat's ledger when peat is present, or always andon's own.
- Pool caps versus a global review-queue cap: the study suggests the review and land queue is the constraint.
- Whether the originating project's two-day stall was process or decision latency; the board would have shown it either way, but the cause is not yet established.
