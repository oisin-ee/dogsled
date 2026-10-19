#!/bin/sh
# A missing binary contributes no context and no shell diagnostic.
command -v dogsled >/dev/null 2>&1 || exit 0
exec dogsled hook context "$@"
