#!/usr/bin/env sh
# Capture the conversation and New conversation screens of each B variant at 1600x1000.
# Usage: .impeccable/mocks/refresh-b/capture.sh [file:prefix ...]
# With no arguments it captures all three variants.
set -eu
dir="$(cd "$(dirname "$0")" && pwd)"
[ "$#" -gt 0 ] || set -- b1-fleet-wall:b1 b2-quiet-console:b2 b3-run-clock:b3
browser() { mise run agent:browser -- "$@" 2>&1 | tail -n 1; }
browser set viewport 1600 1000
for pair in "$@"; do
  file="${pair%%:*}"
  prefix="${pair##*:}"
  browser open "file://$dir/$file.html"
  browser wait 400
  browser screenshot "$dir/$prefix-conversation.png"
  browser open "file://$dir/$file.html?screen=new"
  browser wait 400
  browser screenshot "$dir/$prefix-new.png"
done
browser close
