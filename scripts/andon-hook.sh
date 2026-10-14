#!/usr/bin/env bash
# Put this seat's andon row into the agent's turn: `andon hook`, fed the
# harness's hook JSON on stdin (Claude Code and codex, SessionStart and
# UserPromptSubmit). It prints the row at session start and, on a prompt, what
# changed for the seat since its last turn; nothing when nothing changed.
#
# It runs the workspace's newest built andon (release or debug), else one on
# PATH. It never builds and never fails a session: with no binary it prints
# nothing and exits 0.
root="$(jj workspace root 2>/dev/null || pwd)"
bin=""
for candidate in "$root/target/release/andon" "$root/target/debug/andon"; do
  if [ -x "$candidate" ] && { [ -z "$bin" ] || [ "$candidate" -nt "$bin" ]; }; then
    bin="$candidate"
  fi
done
[ -n "$bin" ] || bin="$(command -v andon 2>/dev/null)"
[ -n "$bin" ] || exit 0
"$bin" hook 2>/dev/null
exit 0
