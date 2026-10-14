---
title: prior art, Scion (agent-scale source control at Anthropic)
date: 2026-10-13
status: living
amends: 2026-10-13-andon-founding.md
---

# Prior art: Scion

Source: Anthropic Engineering, "Redesigning source control at Anthropic", 2026-10-13.
<https://www.anthropic.com/engineering/redesigning-source-control>

Scion is the version control system Anthropic built to replace git in its monorepo, after merges rose from about 2,500 a day (May 2026) to over 30,000 a day (September 2026). It works at a scale andon does not target: one service, tens of thousands of writers, no full copy of the repository anywhere. Its storage and transport are not relevant here. Its diagnosis and several of its principles are, and they arrived independently of the study of one project's agent loop (about 58 landings a day across six seats).

Quotations are verbatim from the article.

## 1. The same diagnosis, one layer down

> "At Anthropic, coding is no longer the bottleneck to building systems or doing research."
> "The bottleneck, now, is all of the developer infrastructure built for human scale: source control, builds, and CI."

The founding design (§1, §2) makes the same claim about coordination: agents pay nothing to create an item, a message or a run, so the friction human organizations get for free must be built in. Scion finds it in storage and transport at 30,000 merges a day. The study finds it in messages, trackers and hub seats at 58. The constraint moves to whatever was sized for people.

## 2. Derived, never maintained

> "Each type of derived data is defined recursively, so its value for a changeset is computed from its values" [at its parents]

Scion computes blame, path history and search from immutable changesets, eagerly or lazily, and can change how without changing clients. That is andon's invariant that every surface is a fold over the event ledger (§4, §6). It also names the implementation shape: a surface defined recursively over an append-only ledger can be cached at any prefix and extended, so `asof` is a lookup of a prefix, not a special mode, and byte-for-byte reproduction follows from immutability of the inputs.

## 3. Global state is the scaling problem

> "global state that every server has to agree on is a looming scaling problem."

Scion discourages refs for this reason. andon's equivalent is the hub seat: one coordinator that every seat must reach, the single-server law in §2 (median reply 989 min, 400 obligations swept). andon keeps no shared status anyone must update. The only shared state is the artifacts each seat already writes, and the board is computed from them.

## 4. Revisions are read through a service, not a checkout

> "The working copy is optional, and sparse by design."
> "A comprehensive source control service can power many novel applications."

Scion lets CI and tools query history and contents with no local repository. For andon this is a constraint on adapters: they read revisions from the object store and the op log, never by materializing a working tree. This bears directly on goal andon-76c (one primitive for reading a frozen revision without a checkout).

## 5. Where the merge queue goes

> "A proper merge queue, for example, doesn't make much sense at agent scale."
> "we've invested in post-merge and pre-deploy automated testing."
> "This does mean that concurrent changes can conflict, but Claude can typically fix these conflicts faster than humans"

This is the one point that pulls against the originating project's design, which gates every landing before publish (median 7.8 min of a 32.8 min change). Little's law says why both can be right: a serialized gate is cheap while arrival rate times gate time stays well under one, and becomes the queue as either grows. Open question §11 already suspects the review and land queue is the constraint. andon does not choose where checks run; it should make the choice visible, by reporting time spent waiting to land separately from time spent in the gate, per seat and per day.

## 6. Review of intent, and the engineer's "no"

> "they now examine the intent of the change as opposed to reading every line of code."
> "the code review tooling we built can express complex semantic constraints on approvals"
> [adversarial AI review] "can also be myopic"
> "Very often, we found that the role of the engineers was to tell Claude "no.""

The goal schema (§3) puts intent where review can check it: a why and a done-when that can fail. andon reads verdicts as facts and does not grade them; whether a critic is too defensive is visible as a RETURN rate, not judged. The refusals (§7) are the "no" encoded at a choke point, which is where the founding design says norms hold and written rules do not.

## 7. Monitors need a machine surface

> "We call these Claudes "superfans.""
> "They have access to our telemetry and logs, and act like an experienced engineer who has nothing better to do"

A monitoring agent is a reader. andon gives it one that is cheap and stable: `--json` and the `hook` row. andon itself stays a fold and does not host the monitor, since judgment is not an input or an output of the board.

## What this does not change

No change to the ledger type, the adapters, the surfaces or the refusals. Section 5 adds one question to §11 of the founding design: whether land wait should be a gauge distinct from gate time.
