---
title: milestone 1, what the build decided
date: 2026-10-13
status: living
amends: 2026-10-13-andon-founding.md
---

# Milestone 1: what the build decided

Milestone 1 (goal andon-xa4) built the ledger, the recorded adapter, the live
adapters for jj, bd, Ferry, sessions and hail, and the surfaces `board`,
`scorecard`, `asof`, `hook`, `ledger` and `check`. These are the choices the
founding design left open or that the build made differently. Each is in the
code; this records why.

## 1. The ledger is andon's own, and the project keeps it

Open question §11 asked whether the ledger should be peat's or andon's.
It is andon's, and andon still writes nothing: `andon ledger` prints it as
JSONL, and a project that wants history redirects that into a file it owns.
`--ledger FILE` reads a kept ledger back in place of the sources.

The reason is that sources lose history. Ferry receipts are deleted with
their workspace; hail's owed sweep removed 400 obligations; `bd export` is
current state, so a claim later released shows only as its present owner.
`asof` over live sources can only be as exact as the sources are; `asof`
over a kept ledger is exact by construction (the study's tests check that a
past board does not depend on anything recorded after it, and that a kept
ledger replays to the same bytes).

## 2. Exit codes

The founding design says a refusal exits non-zero. The build fixes three:

| code | meaning |
|---|---|
| 0 | a surface rendered, or a check passed |
| 1 | a check refused; one line on stdout names the fix |
| 2 | andon could not decide (no config, an unreadable source); one line names what to repair |

Code 2 exists so a check that cannot read its source neither passes silently
nor blocks a landing for the wrong reason. `andon hook` is the exception: a
hook must never break a session, so it prints nothing and exits 0 on error.

## 3. The event type

Two additions to §4. `actor` records who the source says acted, verbatim
(a trailer value, a tracker account, a workspace, a message label); `seat` is
filled afterwards by resolution, so a misconfigured seat map never loses the
raw fact. `create` is a kind: the branching gauge needs items created, and
§4 listed only claim and close. Source-specific facts (recipe, repository,
reason, message kind and recipients) go in `attrs`, a sorted map.

## 4. Sessions come from transcripts, not peat

§5 names peat as the sessions adapter. The build reads the harness
transcripts (Claude Code JSONL, Codex rollouts) directly, as the study did.
peat's ledger is derived from the same transcripts and also holds agent
assertions (`peat obs`), which the invariants exclude as an input; the
transcript is the artifact. Tool-call times become activity spans; a
subagent's effort counts for the seat whose session started it.

## 5. Seat resolution

In order: a configured alias or prefix; the event's session, learned from
session spans and then from any event whose seat is known; and a workspace
whose sessions all belong to one seat (a seat's task-named extra desks).
With no `[seats]` configured, every actor is its own seat. The study's join
rates reproduce: 117 of 117 landings attributed through their sessions.

## 6. Reading without writing

jj runs with `--ignore-working-copy --at-op @`: no workspace snapshot, and no
merge of divergent operation heads, either of which would record an
operation. bd is read through `bd export`, which writes nothing. A landing's
time is the operation that first pointed the trunk at it or a descendant,
but only for commits made after the trunk's previous move; older commits
fall back to committer time rather than inheriting a later batch's time.

## 7. What the study's numbers turned out to mean

Pinning every headline number with a definition showed two things the
study's README states loosely:

- "883 runner runs, 434 failed, 364 with no kept reason" mixes two scopes.
  883 is every run from lanes starting with the project's name or `ferry`;
  434 and 364 are runs whose receipts name the project's repository, from
  any lane. Over one scope the numbers are 883/445/371 and 826/434/364.
  Both are pinned.
- "969 open, 849 unowned" is the tracker at the snapshot (read during
  2026-10-13), not at the window's end (00:30), when 850 were unowned.

## 8. Identity is the weak join, here too

andon's own first claim recorded bd's default actor, the shared account,
as the owner, which no seat map can resolve: the board showed the seat
holding no goal. This is the study's 19%-solid comment join, reproduced on
day one. Until each seat's claims carry its own name (`BEADS_ACTOR` per
workspace, or `--assignee` on claim), `check commit` refuses a seat's
landings as "not claimed by" it. Milestone 2's wiring has to settle it.

## 9. Open

- The hub's reply gauge is the first reply of any kind to a message into the
  hub (study: about 16 hours); the study's 989 minutes was ask → done.
  Whether a reply means `done` should be configuration.
- Gate runs (the land verb's own records) have no live adapter: the study's
  came from the originating project's gate spool, a project-specific store. Until one exists,
  the board's "last land refused" flag fires only on recorded ledgers.
- §5 of the Scion amendment asks for land wait as a gauge distinct from gate
  time; it needs gate runs first.
