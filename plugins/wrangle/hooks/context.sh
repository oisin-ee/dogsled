#!/bin/sh
# A missing binary contributes no context and no shell diagnostic.
command -v wrangle >/dev/null 2>&1 || exit 0
exec wrangle hook context "$@"
