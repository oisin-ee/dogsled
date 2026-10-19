#!/bin/sh
# Popup board: every host, its agents, and its reservations.
set -eu
. "$(dirname "$0")/dogsled.sh"
run_dogsled status --watch
