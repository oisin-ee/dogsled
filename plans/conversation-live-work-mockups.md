# Conversation live-work mockups

## Scope and status

The owner selected an inline live-work block with a compact dock when the live turn leaves the viewport.
This deliverable contains mockups only. No production behavior, activity producer, or data contract changed.
The owner has not approved the rendered mockups for implementation.

The mockups preserve the dark console, timeline, inbox, and pinned composer.
The dock repeats the active task and current action, not the full agent update.

## Preview files

All files are in `.impeccable/mocks/conversation-live-work/`.

| File | View | Capture |
| --- | --- | --- |
| `inline.html` | Current task, recorded agent update, and tool activity | `inline.png`, 1600 × 1000 |
| `dock.html` | Earlier messages with the compact activity dock | `dock.png`, 1600 × 1000 |
| `narrow.html` | Thinking phase with the inbox visible and other columns folded | `narrow.png`, 900 × 800 |

Open the HTML files locally. Native disclosures expand the agent update and tool details.
The previews do not send messages or interrupt agents. Their disabled controls show the intended placement.
The dock and inline view are separate state mockups. Automatic scroll-based switching is not implemented.

`review.json` presents the three views through Impeccable:

```sh
/Users/oisin/.pi/agent/skills/impeccable/scripts/impeccable serve-question --start --payload .impeccable/mocks/conversation-live-work/review.json
```

Impeccable can require new captures after a new review round starts.
The source HTML files remain usable without its server.

## Content sources

The owner questioned how the original mockup summaries would be generated.
The revised mockups remove the implied summarization service and inferred tool findings.
Every message, time, duration, and task shown in these files is illustrative.

| Display | Proposed source | Boundary |
| --- | --- | --- |
| Work heading | Active todo title | Do not invent a current purpose from tool names. |
| Agent update | Existing assistant commentary | Omit it when unavailable. Do not generate replacement prose. |
| Thinking phase | Reported thinking event | A phase does not establish what the agent is thinking about. |
| Tool action | Tool name and allowlisted target fields | Show a file path for a file read. Do not infer findings. |
| Duration and outcome | Correlated start/end events | A recorded result does not prove task success. |

Existing harness-exposed reasoning or summaries can be considered separately after their availability is verified.
These mockups do not promise a generated reasoning summary or require another model call.
Arbitrary tool arguments, command text, and output require a separate disclosure policy before production exposure.
The command in the expanded mockup is illustrative, not approval to publish raw shell arguments.

Dogsled's current publisher sends phases and tool names, not these display fields (`pi/activity.ts:389–418`).
Its shared live state has no summary field (`portal-types/src/agents.rs:245–269`).
Implementation therefore needs a source-backed data contract before these views can become live.
Telemetry loss must retain the last known activity with an explicit gap, not imply completion.

## Method and evidence

Impeccable context was loaded for `web/src/agent_view.rs` in this session.
The direct image command could not run because its API key was absent.
The supported Pi image path returned an authentication error and produced no images.
The `.prompt.txt` files preserve those failed generation requests; they do not describe successful image generation.

The final previews use Impeccable's supported HTML path.
Its preview response disables scripts, so the final files use declarative HTML and native disclosures.
No preview security setting was changed. PNG files are browser screenshots, with provenance recorded in their sidecars and metadata.

The managed browser inspected all three views. At 900 × 800, the document had no horizontal overflow or document scrolling.
The active action and footer fit above the composer. Expanding tool details used the history's own scroll region.
Native tool and agent-update disclosures opened and closed correctly.

The Impeccable detector ran on all three HTML files.
The review corrected small type sizes, an undeclared font fallback, palette drift, and a skipped heading.
The header padding warning did not account for its centered fixed-height layout; explicit vertical padding was added.
The remaining animated indicator is an illustrative live-state marker, not a claim of real telemetry.
Reduced-motion preferences disable its animation. No clean detector result is claimed after the correction batch.

Production Rust and TypeScript tests are outside this artifact-only change.
The Git hook selects checks affected by the staged files.

## Before implementation

Confirm the revised visual treatment and the source policy with the owner.
Update the feature's editable Squig layout before production layout work.
Define supported harness data and missing-data states before implementing the UI.
Use the existing Dioxus components in production; these HTML files are design artifacts only.
