---
title: what of the goal schema is code, and what is config
date: 2026-10-19
status: decided
amends: 2026-10-13-andon-founding.md
---

# The goal schema: code and config

The user asked whether goals should be configuration rather than code
(andon-zeh). Ruling: andon keeps the goal as a concept in code, and keeps
everything about how a project writes one in config.

**Code fixes four things.** A goal:

- exists in a tracker, as an item andon can read;
- has an owner, set by a claim, never by hand;
- has a done-when that can fail;
- has a finite budget.

These are the founding design's §3 claims. Without them, `check commit`
has nothing to cite, WIP has nothing to count and `over budget` has nothing
to compare. They are what andon is for, so no project can switch them off
piecemeal.

**Config holds the rest.** `[goal]` already holds:

- the section names (`sections`, `done_when`, `budget`);
- the tracker fields that stand in for a section (`fields`);
- the stems that make a done-when look checkable (`checkable`), which is
  advice and never a refusal;
- WIP per seat and per seat override, the item types that count, and
  whether a pool is required;
- the budget unit, which lives under `[board]`.

**A project that declares no `[goal]` section gets no goal checks.** Today
a missing `[goal]` silently applies the founding schema, so a repository
adopting andon only for its board meets refusals and "no budget" columns
for a schema it never chose. From andon-3eu on, absence means:

- `check goal` passes with a one-line note that no schema is configured;
- `check commit` requires no goal citation;
- the board drops its goal and budget columns, instead of printing them
  as unknown.

Declaring `[goal]`, even empty, opts in to the founding defaults.

Considered and rejected: a fully declarative goal (arbitrary fields and
rules in TOML). It would make the four claims above optional, and andon's
refusals would then mean something different in every project.
