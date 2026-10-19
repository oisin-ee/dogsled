#!/bin/sh
# Finds the dogsled binary for a Herdr plugin command. Herdr runs plugin commands with the
# server's environment; on a host where mise shims are not on that PATH, go through mise.
# Usage: . plugin/dogsled.sh; run_dogsled <args...>
run_dogsled() {
  if command -v dogsled >/dev/null 2>&1; then
    exec dogsled "$@"
  fi
  if command -v mise >/dev/null 2>&1; then
    exec mise x -- dogsled "$@"
  fi
  if [ -x "$HOME/.local/bin/mise" ]; then
    exec "$HOME/.local/bin/mise" x -- dogsled "$@"
  fi
  echo "dogsled: binary not found on PATH or through mise" >&2
  exit 1
}
