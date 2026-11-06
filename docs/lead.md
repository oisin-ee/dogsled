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
A lead stays open until an explicit `dogsled close lead-<id>` request closes its idle pane.

## MCP and HTTP

The MCP `lead` tool accepts `repo: "owner/name"` and `message: "the first prompt"`.
The tool returns the same lead result as the CLI.

`POST /api/leads` accepts this body:

```json
{"repo":"owner/name","prompt":"Plan the next release"}
```

After the lead accepts its first prompt, the endpoint returns `201` with `host` and `pane`.
Invalid input returns `400`. Unavailable launch capability returns `503`. A launch or prompt failure returns `502`.
The endpoint does not retry an uncertain launch or close a partially started pane.
