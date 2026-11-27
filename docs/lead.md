# Start a lead

```sh
dogsled lead owner/name "Plan the next release"
dogsled lead owner/name --message-file prompt.md
dogsled lead owner/name --message-file - < prompt.md
dogsled lead owner/name "Plan the next release" --json
```

The repository argument is a GitHub slug, such as `oisin-ee/dogsled`. Paths and repository URLs are not valid arguments.

Dogsled chooses the reachable fleet host with the most free room from the service publication. You do not select a host.
The score and tie breakers match worker placement. Leads do not use capacity, reservations, or the queue.
When every reachable host exceeds its admission limit, Dogsled still chooses the host with the most room.
An unavailable publication or a fleet without published headroom returns an error.

On the selected host, Dogsled reuses a matching main checkout under `~/dev`. SSH and HTTPS origins can identify the same repository.
Dogsled never changes an existing checkout's branch.
If no checkout matches, Dogsled clones `git@github.com:owner/name.git` into `~/dev/name`.
If that path already exists, Dogsled uses `~/dev/owner-name`. If both paths are occupied, the request fails without changing either path.

The selected host keeps the lead record and its Herdr pane. The result includes `host`, `pane`, and `name`.
Dogsled sends the first prompt once. An uncertain start or lost peer response does not trigger another host selection or delivery.
Pi summarizes the full opening message into a short session name, such as `login-redirect-loop`. Later messages do not change it.
If naming fails, the lead still starts with its request ID instead of copying prompt text.
A `-<n>` suffix keeps it unique on its host.
A lead stays open until an explicit `dogsled close <name>` request closes its idle pane. The pane and `lead-<id>` also name the lead.

## MCP and HTTP

The MCP `lead` tool accepts `repo: "owner/name"` and `message: "the first prompt"`.
The tool returns the same lead result as the CLI.

`POST /api/leads` accepts this body:

```json
{"repo":"owner/name","prompt":"Plan the next release"}
```

The request must include one `Idempotency-Key` header with 1 to 128 visible ASCII characters.
The endpoint returns `202 Accepted` as soon as it saves the request. The response contains a lead start receipt, not the prompt.
The service runs the start separately. The lead does not need to accept its first prompt before the endpoint returns.

The same key and body return the current receipt. The same key with a different body returns `409 Conflict`.
A missing or invalid key or body returns `400`. Unavailable placement or launch capability returns `503`.
Other placement failures can return `502`. A store failure returns `500`.

The `lead_starts` field of the portal publication carries the state, name, phase, and pane address when known.
The service publishes up to 50 receipts, newest first. It includes finished receipts for 10 minutes after their last update.
The service does not retry an uncertain launch or close a partially started pane.
