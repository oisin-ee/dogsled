#!/bin/sh
# Right-side board. Plugin context identifies the pane that opened it.
set -eu
. "$(dirname "$0")/dogsled.sh"
run_dogsled board --side --watch
