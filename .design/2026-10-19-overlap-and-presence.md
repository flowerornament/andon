---
title: overlapping edits and seat presence, from what harnesses already write
date: 2026-10-19
status: decided
amends: 2026-10-13-andon-founding.md
---

# Overlapping edits and seat presence

A study of flopwire (github.com/flopwire/flopwire, read 2026-10-19), a
messaging and history tool, found two facts that harnesses already write
and andon does not read. Each closes a hole in one of the founding five
coordinations.

## 1. Overlapping unlanded edits (ownership)

The founding design's first coordination is that the same work is not done
twice. andon supports it only by refusing a commit whose goal another seat
holds. Two seats can still hold different goals and edit the same file.
Each workspace is isolated, so neither sees the other, and the conflict
surfaces only at land, as a rebase.

A transcript records every edit tool call with its path: Claude Code's
`Edit`, `Write`, `MultiEdit` and `NotebookEdit`, and the paths in Codex's
`apply_patch`. These are artifacts, not assertions.

- The sessions adapter emits `edit` events. Each carries the seat (as
  `session_active` does) and the path. A path under the session's
  workspace root is made relative to that root, so `andon-1a/src/x.rs` and
  `andon-1b/src/x.rs` are one file. A path outside every configured
  workspace is kept absolute and never matched.
- A seat's **unlanded edits** are the files it edited after its last
  landing (its last `land` event, or the window start when it has none).
  A seat with no landing in the window counts from the window start.
- The board flags a seat whose unlanded edits share a file with another
  seat's: `overlaps andon-1b on 3 files (src/render.rs, ...)`. `--json`
  lists every shared path. The hook prints the line when an overlap is new
  since the seat's last turn.
- An overlap is a fact, not a refusal. Two seats may share a file on
  purpose. Coverage is the sessions adapter's, so an unreadable transcript
  makes the flag `?`, never absent.

What it cannot see: edits made by shell commands (`sed -i`, a formatter,
a code generator). The flag can miss an overlap, and the help says so.

## 2. Presence (observability)

The founding design wants stuck and done to be visible without asking. The
board shows a seat's last landing and its activity spans, but not whether
a session is running there now, or has been waiting on a human for hours.

Claude Code writes one `sessions/<pid>.json` per running session, under
its config directory. Each file names the session id, the cwd, the tmux
pane, and `status` (`busy` or `idle`) with `statusUpdatedAt`. A file whose
pid is not running, or is running but started at another time than
`procStart`, belongs to a session that has ended.

- A new `presence` adapter reads these files at capture and emits one
  current-state event per live session: kind `presence`, the seat from
  the cwd, status, and since. `t` and `t_end` are both the capture instant,
  as bd's current-state claims are. `asof` a past instant therefore shows
  presence as unknown, which is the coverage rule working as intended.
- Checking the pid is the one process-table read in andon: one `ps -A -o
  pid= -o lstart=` through `adapters::command`, with `TZ=UTC`, because the
  harness records `procStart` in UTC and `ps` prints local time. andon
  writes nothing.
- The board gains a `state` column (`busy 4m`, `idle 3h`, `—` for no live
  session) when `[adapters.presence]` is configured; without it there is
  no column, not a column of unknowns. A seat with several live sessions
  is busy while any is. The board also flags a seat that has been idle
  longer than `[board] idle_minutes` (default 60) while it holds a goal:
  `idle 3.0h holding andon-xyz`. That is the stuck seat the study's
  coordinator had to ask about.
- The hook does not read presence: it prints its own seat's row, and that
  seat is running by definition.
- Codex keeps no equivalent status file. Its sessions show as unknown
  until a source exists, and the adapter's coverage says so.

## Not taken

Flopwire's team server, transcript search and messaging belong to peat and
hail, or to neither. Its delivery facts (undelivered, session ended)
become andon inputs once hail records them; the hail adapter reads them
then.

## Amendment: overlaps as built (2026-10-20, andon-tgv)

The flag is driven by jj, not by transcripts. On the live andon config,
neither seat's edits showed up in its transcripts. Both seats edit through
the shell, and a coordinator's session was attributed to the colocated
checkout, so its probe edit of `andon-1a/CHANGELOG.md` stayed absolute and
never matched. jj already records the fact whatever tool wrote the file.

- **Source.** The jj adapter emits `unlanded` for each file in a
  workspace's net change from `fork_point(trunk | <ws>@)` to `<ws>@`, with
  the workspace's seat. A change undone is not in the net change. A
  landing empties the landed side. Paths are the repository's own. Each
  fact's `t` is when the newest commit in that range touching the file was
  written, `t_end` is the capture, and `snapshot` is the workspace's newest
  operation. The read uses `--ignore-working-copy`, so each workspace is as
  of its last snapshot: minutes old wherever agents run jj.
- **Current state.** Every workspace is covered as `state:unlanded` at the
  capture instant. An unreadable workspace is a gap there for every seat,
  since an overlap needs them all. `asof` an earlier instant is unknown,
  and the text then says nothing of overlaps, as it does without jj.
- **The hook.** The workspaces are read only at the capture. An overlap is
  news when a shared file changed on either side after the last turn:
  before then, both sides already held it. No narrow read of other seats'
  transcripts is needed.
- **Transcripts.** The sessions adapter still emits `edit` for edit tool
  calls, relative to the session's workspace root, for per-session
  attribution. No gauge reads it yet.
- **git.** The analogue is each worktree's `git diff --name-only
  <trunk>...HEAD` plus its uncommitted changes. It is not built.
