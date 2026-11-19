#!/usr/bin/env sh
# Capture both screens of one or more directions at 1600x1000.
# Usage: .impeccable/mocks/refresh/capture.sh a-briefing:a b-live-fleet:b
set -eu
dir="$(cd "$(dirname "$0")" && pwd)"
browser() { mise run agent:browser -- "$@" 2>&1 | tail -n 1; }
browser set viewport 1600 1000
for pair in "$@"; do
  file="${pair%%:*}"
  prefix="${pair##*:}"
  browser open "file://$dir/$file.html"
  browser screenshot "$dir/$prefix-conversation.png"
  browser open "file://$dir/$file.html?screen=new"
  browser screenshot "$dir/$prefix-new.png"
done
browser close
