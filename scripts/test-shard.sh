#!/bin/sh
# Run one of two halves of `cargo test`, so that each half fits a CI runner's timeout.
# Shard 1 runs the CLI test modules in the list below. Shard 2 runs every other test:
# the unit tests, the plugin tests, and the remaining CLI modules. Both shards read the
# same list, so together they run each test exactly once.
# Shard 2 also builds and runs the unit tests, so the list gives shard 1 more of the CLI
# suite. Keep the two macOS jobs about equally long when a module grows.
# libtest matches each name as a substring. `sweep::` therefore also selects
# `stop_sweep::`. That is correct, because shard 2 skips the same substrings.
set -eu

modules="sweep:: launch_retry:: jobs:: preservation:: close:: signals:: unit_seeded:: legacy_continue::"

case "${1:-}" in
1)
	# shellcheck disable=SC2086 # Each module is a separate filter.
	exec cargo test --test cli -- $modules
	;;
2)
	cargo test --bins --test plugins
	skips=""
	for module in $modules; do
		skips="$skips --skip $module"
	done
	# shellcheck disable=SC2086 # Each skip is a separate argument.
	exec cargo test --test cli -- $skips
	;;
*)
	echo "usage: $0 1|2" >&2
	exit 2
	;;
esac
