#!/bin/sh
# Diagnostic only: run shard 2 exactly as CI does, and sample any long-lived process
# under the test binary, with its children.
set -u
cargo test --no-run --bins --test plugins --test cli
sh scripts/test-shard.sh 2 &
tests=$!
: >/tmp/sampled
while kill -0 "$tests" 2>/dev/null; do
	sleep 15
	ps -axo pid=,ppid=,etime=,command= >/tmp/ps.now
	# Processes older than 90 s whose command names the rig or the dogsled binary.
	awk '{ split($3, t, /[-:]/); n = length(t); s = t[n] + 60 * t[n - 1] + (n > 2 ? 3600 * t[n - 2] : 0) + (n > 3 ? 86400 * t[1] : 0);
		if (s >= 90 && ($0 ~ /target\/debug\/dogsled/ || $0 ~ /\/rig/ || $0 ~ /herdr|lsof|git /)) print $1 }' /tmp/ps.now | while read -r pid; do
		grep -qx "$pid" /tmp/sampled && continue
		echo "$pid" >>/tmp/sampled
		echo "=== STALL $(grep -E "^ *$pid " /tmp/ps.now)"
		awk -v p="$pid" '$2 == p' /tmp/ps.now | sed 's/^/=== child /'
		sample "$pid" 2 -mayDie 2>/dev/null | sed -n '/Call graph/,/Total number in stack/p' | head -60
	done
	if [ "$(wc -l </tmp/sampled)" -ge 8 ]; then
		echo "=== enough samples"
		cat /tmp/ps.now
		kill "$tests"
		break
	fi
done
wait "$tests"
