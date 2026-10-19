#!/bin/sh
# Hand this pane's dogsled inbox to Claude Code or Codex. A missing binary hands over nothing.
command -v dogsled >/dev/null 2>&1 || exit 0
exec dogsled hook inbox "$@"
