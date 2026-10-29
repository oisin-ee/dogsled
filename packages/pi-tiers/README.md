# @oisin-ee/pi-tiers

Tier virtual models keep each branch on its chosen physical model until a failure requires recovery.
The selected thinking level passes through to the physical model. Pi clamps unsupported levels.
A model switch loses prompt caches and thinking signatures.

## Config

Put `tiers.json` in the Pi agent directory (`~/.pi/agent` by default):

```json
{
  "sol": ["cliproxy/gpt-5.6", "cliproxy/claude-opus-4-6"],
  "luna": ["cliproxy/minimax-m3", "cliproxy/gpt-5.6"]
}
```

Select `tier/sol` or `tier/luna`. Each target must identify a configured physical model.
An absent file registers no tiers. Invalid config stops registration. Reload Pi after config changes.
This package does not load `model-alias.json` or register `alias/*` models.

## Recovery

Gateway failures open a provider-wide circuit. Recovery waits, then retries the same target.
The delay starts at two seconds and doubles to one minute. Gateway failures never cool individual models or pools.

Quota failures exclude a pool. Model IDs that start with `claude-`, `gpt-`, or `minimax-` use the corresponding shared pool.
Other models use their provider ID as the pool.
The snapshot reset time controls the exclusion when the pool is exhausted.
Without that reset, cooldown starts at five minutes and doubles to one hour.

Pi handles native transient retries first. After native retries end, boundary recovery selects the next target.
Unknown models and request, authentication, or configuration failures stop without failover. Abort never requests recovery.
Each user turn permits two recoveries. Native transient retries retain Pi's separate retry budget.

Boundary recovery removes only the failed assistant entry from model context.
A state-only custom entry records the next route. Recovery never creates user messages or rewrites their text.
The chosen target, attempted targets, and remaining budget follow the session branch.

## Quota and shared state

Before routing, the extension runs `dogsled quota --json --cached` with a two-second timeout.
The result stays in memory for one minute. Missing Dogsled or an invalid snapshot disables quota filtering.
The command never requests a CLIProxy refresh.

A pool is exhausted only when every account has a full window with a future reset.
For multiple blocked windows, an account becomes available after its last reset.
The first available account ends the pool exclusion.

The shared store is `~/.pi/agent/state/pi-tiers/state.json`.
An exclusive `lock/` directory protects each read-modify-write transaction. Its `owner.json` records the process ID and creation time.
A temporary state file and atomic rename protect readers. Lock acquisition stops after two seconds.
A dead owner or a lock older than 30 seconds permits takeover through atomic directory rename.
An ownerless directory also expires after 30 seconds. Retired generations remain for 60 seconds to exclude competing takeovers.
Invalid state and filesystem failures stop routing rather than discard shared health.

Pi 1.3.0 response events contain no model or request identity.
HTTP status applies only when an error includes the response's `x-request-id` or `request-id` value.
Otherwise, the classifier uses error text. Unrelated response status never affects routing.

## Development

```bash
proto run moon -- run pi-tiers:lint pi-tiers:test
```
