#!/bin/sh
# Hand this pane's wrangle inbox to Claude Code or Codex. A missing binary hands over nothing.
command -v wrangle >/dev/null 2>&1 || exit 0
exec wrangle hook inbox "$@"
