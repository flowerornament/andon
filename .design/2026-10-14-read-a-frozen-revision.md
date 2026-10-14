---
title: one primitive for reading a frozen revision
date: 2026-10-14
status: decided
goal: andon-76c
---

# Reading a frozen revision

Every tool that supervises agents on one repository reads artifacts as of
a revision: a candidate under review, a landing, a pinned build input. On
one machine running six such tools, each read a frozen revision its own
way. A census of about 80 sites found one class of failure behind every
incident. It is kept on the goal, because it names other projects.

- **Discovery walks up.** A bare `git` call, or an unchecked
  `--show-toplevel`, in a jj-only workspace finds the nearest enclosing
  repository, which on such machines is often the home directory's. The
  answer is then silently about the wrong repository. This was the most
  common defect: it appeared in seven tools' code and scripts.
- **A read writes.** jj without `--ignore-working-copy` snapshots the
  working copy it is pointed at. Pointed at a shared anchor, it records
  another agent's half-made edits. jj without a pinned operation merges
  divergent operation heads, which is also a write. `git status` in another
  worktree refreshes its index.
- **A read moves `@`.** Procedures that review by `jj new <id>`, or refresh
  an anchor by `jj new main`, move a working copy that others read. An extra
  workspace per review left hundreds of checkouts behind.
- **No single state.** Several separate reads of `@` or a symbolic name each
  see a different state. A colocated checkout's git refs lag jj's bookmarks.
- **A tree where none is needed.** Most sites only list a tree, read blobs,
  read a log or diff two commits. A few build at the revision. Those few are
  the only ones that need a working tree, and a fresh private one.

## The primitive

**Read revision R** is four steps, each with one rule.

1. **Locate.** Start from a named directory and stop at the nearest `.jj`
   or `.git`. A git top level must equal that boundary, or the repository
   is unavailable, never a parent's. Git is addressed by explicit
   `--git-dir`: the backing store a jj workspace names
   (`.jj/repo/store/git_target`), or the validated `.git`. `GIT_DIR`,
   `GIT_WORK_TREE`, `GIT_INDEX_FILE` and `GIT_COMMON_DIR` are removed, and
   `GIT_OPTIONAL_LOCKS=0` is set.
2. **Pin.** Resolve the operation once, and read at it with
   `--ignore-working-copy --at-op`. Divergent heads are read at the newest,
   never merged. Resolve R once to one full commit id, and refuse a revset
   naming none, several, or a conflicted commit. `@` means the caller
   workspace's recorded working-copy commit. It is read without a snapshot,
   and a stale workspace is reported, not repaired.
3. **Read.** Every query (tree listing, blob bytes, log and trailers, diff
   of two pinned ids) goes to the object store by full id. No working tree
   is involved.
4. **Materialize, for builders only.** `git archive <id>` goes into a fresh
   private directory. The files are verified against `ls-tree`, refused if
   a build modified its own sources, and removed when done. A remote build
   keeps its runner's isolated seat, which takes its commit from step 2.

What each prior implementation contributes:

- the best locator: stop at the nearest marker, validate the boundary,
  scrub the environment, address the backing store explicitly, and
  re-check the pin;
- andon's operation pinning: one `--at-op` for the whole read, with no
  merge write;
- one tool's archive path, which is the materializer;
- the runner's discipline: one resolution, then everything keys on the
  commit id.

## Its home

A small Rust library with a thin command line, in its own repository and
owned there. It is seeded from the best locator and from andon's pinning.

- **Not andon.** andon is a read-only signal board and one caller among
  many. Build tools must not depend on it to read their own inputs.
- **Not the runner.** The runner executes with custody on a host, while
  most sites need no host and no tree. The runner calls the library to
  resolve its commit, and keeps its seats for builds.

## What changes in andon

andon adopts the library's locator for both its jj and git adapters, when
the library exists. Until then its git adapter takes the same rules: an
explicit top-level check against the configured path, and a scrubbed
environment. Its jj adapter already pins and never snapshots. The rest is
each site's own migration, listed on the goal. No rewrite is part of this
decision.
