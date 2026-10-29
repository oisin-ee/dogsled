# @oisin-ee/oisinpi

Oisin's [pi](https://pi.dev) packages in one install, at versions tested together:

| Package                                                                            | Contents                                                                   |
| ---------------------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| [`@oisin-ee/pi-prewalk`](https://www.npmjs.com/package/@oisin-ee/pi-prewalk)       | A frontier model plans and makes the first edit; a cheaper model finishes. |
| [`@oisin-ee/pi-todo`](https://www.npmjs.com/package/@oisin-ee/pi-todo)             | The `todo` tool, which also opens the prewalk gate.                        |
| [`@oisin-ee/pi-guardrails`](https://www.npmjs.com/package/@oisin-ee/pi-guardrails) | Git safety, the wait gate, and the agent-typing gate on every tool call.   |

```bash
pi install npm:@oisin-ee/oisinpi
```

The collection bundles the three packages in its tarball and adds no code. No member imports another, so each one also
installs by itself.

pi-dogsled is not in the collection. Install it on its own, at the same version as the `dogsled` binary
([oisin-ee/dogsled](https://github.com/oisin-ee/dogsled)):

```bash
pi install npm:@oisin-ee/pi-dogsled@<dogsled version>
```

When pi-dogsled is active, its work widget hosts pi-todo's view. Without it, pi-todo keeps its own widget.
