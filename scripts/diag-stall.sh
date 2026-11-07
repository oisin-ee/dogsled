#!/bin/sh
# Diagnostic only: run shard 2's CLI tests and sample any long-lived dogsled process.
set -u
modules="sweep:: launch_retry:: jobs:: preservation:: close:: signals::"
skips=""
for module in $modules; do skips="$skips --skip $module"; done
cargo test --test cli --no-run
# shellcheck disable=SC2086
cargo test --test cli -- $skips &
tests=$!
sampled=""
while kill -0 "$tests" 2>/dev/null; do
	sleep 20
	ps -axo pid=,etime=,command= | while read -r pid etime command; do
		case "$command" in
		*target/debug/dogsled*) ;;
		*) continue ;;
		esac
		case "$etime" in
		0[2-9]:* | [1-9][0-9]:* | *-*:*) ;;
		01:[3-5]?) ;;
		*) continue ;;
		esac
		case " $sampled " in *" $pid "*) continue ;; esac
		echo "=== STALL pid $pid etime $etime: $command"
		ps -axo pid=,ppid=,stat=,etime=,command= | awk -v p="$pid" '$2 == p || $1 == p'
		sample "$pid" 2 -mayDie 2>/dev/null | sed -n '/Call graph/,/Total number in stack/p' | head -80
		echo "$pid" >>/tmp/sampled
	done
	sampled=$(cat /tmp/sampled 2>/dev/null)
	if [ "$(wc -l </tmp/sampled 2>/dev/null || echo 0)" -ge 6 ]; then
		echo "=== enough samples"
		ps -axo pid,ppid,stat,etime,%cpu,command | head -400
		kill "$tests"
		break
	fi
done
wait "$tests"
