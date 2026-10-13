#!/bin/sh
# Popup board: every host, its agents, and its reservations.
set -eu
. "$(dirname "$0")/wrangle.sh"
run_wrangle status --watch
