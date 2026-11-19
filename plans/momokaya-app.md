# Dogsled as a momokaya-template app

Status: proposed. No part of this plan is approved or implemented.

## Goal

Dogsled becomes a full momokaya-template app. It has a Crux core, a Dioxus web shell, iOS and Android shells, a Loco API,
and the template's development stack. It takes template updates with `copier update`.

The CLI, `dogsled serve`, the Pi extension, and the harness plugins keep their behaviour and their release channels.

## Decisions

The owner made these decisions on 2026-11-19.

1. **Full app.** Dogsled adopts the whole template. A reduced "tool" shape of the template is rejected.
2. **The Loco API is a fleet gateway.** It runs on one host. It signs users in, and it talks to `dogsled serve`. It does not
   replace the axum API of `dogsled serve`.
3. **Private network only.** Phones reach the gateway through the tailnet. The template's sign-in also applies.
4. **The native apps start with "needs you".** The first scope is the inbox, questions, plan reviews, a message to an agent,
   and host load. Full portal parity is not the first scope.

Decision 3 changes `PRODUCT.md`. It says now that the portal is local only, and that remote and phone access are deferred.
The plan updates that text in unit D5.

## Evidence

Checked on 2026-11-19 at Dogsled `4eb6bb7` and momokaya-template `v1.9.3` (`05a05b3`).

| Finding | Evidence |
| The template always generates the core, the contract crate, the Loco API, iOS, Android, Maestro E2E, `brand/` and the dev stack. Only sign-in, OpenFGA, the site and the web shell kind are options. | template `copier.yml`, `template/` |
| The Dioxus web shell pins `dioxus = "=0.8.0-alpha.3"`. Dogsled's portal uses `dioxus = "0.7.10"`, `dioxus-sdk-time = "0.7.0"`, and `dioxus-primitives` from the DioxusLabs/components Git repository. | template `template/apps/web/Cargo.toml.jinja`; `web/Cargo.toml` |
| The portal is about 12,000 lines of Rust in `web/src`. Its state lives in the components. | `wc -l web/src/*.rs` |
| `portal-types` holds the portal's request and reply types. | `portal-types/src/` |
| `dogsled serve` binds `127.0.0.1`. It rejects a foreign `Host`, and a state change without a same-origin `Origin`. A `Bearer` token passes the `Origin` rule. | `src/web/guard.rs` |
| The portal token is a private file in the ledger directory. | `src/web/token.rs` |
| `dogsled serve` already reads the other hosts over SSH. Agent routes carry the host: `/api/agents/{host}/{pane}/...`. | `src/hosts.rs`, `src/web/peer_stream.rs` |
| `DOGSLED_STATE` and `DOGSLED_CONFIG` select the state and the configuration of one Dogsled. | `src/` |
| Crux 0.20 has streaming effects: `Command::stream_from_shell`. The portal's SSE streams (`/api/stream`, `/api/agents/.../events`) can be shell streams. | [crux_core 0.20.0 `Command`](https://docs.rs/crux_core/0.20.0/crux_core/command/struct.Command.html) |
| The agent hosts run Tailscale. `tailscale0` and `lo` are the only trusted interfaces. | infra `flake.nix:650-652`, `modules/agent-host.nix` |
| The root crate is the binary (`members = [".", ...]`). The root `package.json` is the published `@oisin-ee/pi-dogsled`. | `Cargo.toml`, `package.json` |
| cargo-dist builds the binary from the root crate. Its release workflow is `.github/workflows/release.yml`. | `dist-workspace.toml` (`members = ["cargo:."]`), `.github/workflows/release.yml` |
| The template manages `.github/workflows/release.yml`. It starts a project-owned `.github/workflows/publish.yml` on each new tag. | template `template/.github/workflows/release.yml` |
| A `copier update` merges the root `Cargo.toml`, `package.json` and `pnpm-workspace.yaml`. `_exclude` protects only sources, docs and configuration directories. | template `copier.yml` |
| The template workspace sets `license = "UNLICENSED"`. Dogsled is MIT. Dogsled keeps stricter workspace lints. | template `template/Cargo.toml.jinja`; `Cargo.toml` |
| The template adoption method: generate a baseline with Copier, reconcile, commit `.copier-answers.yml`, then prove a real update in a disposable copy. Copier keeps deleted template paths. | template `plans/template-adoption.md` |

## Design

### Shape

```
apps/
  dogsled/      the CLI and `dogsled serve` (the current root crate)
  api/          the Loco fleet gateway (new)
  web/          the Dioxus 0.8 web shell on the core (replaces web/)
  ios/ android/ the native shells, "needs you" scope (new)
  e2e/          Maestro flows (new)
crates/
  dogsled-contract/  the API contract (grows from portal-types)
  dogsled-core/      the Crux core (new)
  todo-*/ prewalk-*/ unchanged
packages/
  pi-dogsled/   the Pi extension (the current pi/ and root package.json)
  oisinpi/ pi-guardrails/ pi-prewalk/ pi-tiers/ unchanged
brand/          the dark console palette as DTCG tokens
```

### The contract

`dogsled-contract` is the one owner of the portal API types. It starts as `portal-types`. It gains the typed replies and
the error model of the template (template ADR 0001). `dogsled serve`, the gateway and the core all use it.

### The gateway

The gateway is a Loco app on one host. It binds the host's tailnet address and loopback only. It signs users in with the
template's email-link sign-in. It forwards each contract request to the `dogsled serve` on its own host with the portal
token. That `dogsled serve` already reads the other hosts, so the gateway needs no SSH and no host list.

The gateway owns sign-in, sessions and the forward. It owns no agent state. A request that `dogsled serve` refuses keeps its
contract error.

### The core

The core owns the portal's state: the board, agents, conversations, the inbox, questions, reviews, metrics and leads. It
asks the shell for HTTP requests and for SSE streams through `Command::stream_from_shell`. Error copy and recovery actions
live in the core, as in the template.

### The shells

The web shell is the template's Dioxus shell. The native shells first render the "needs you" view models only. The web
shell renders every view model. A view model that a native shell does not render yet stays in the core.

## Open decisions

The owner decides these before the named unit starts.

| # | Decision | Options | Recommendation | Before |
| O1 | Which host runs the gateway | `local` (netcup), `momokaya-2`, `local-mac` | netcup: it is always on, and it is not a laptop | D5 |
| O2 | Who serves the web shell | (a) the gateway only, with sign-in; (b) the gateway, and `dogsled serve` also embeds it for loopback use with the token | (b): the portal works when the gateway is down. The core then has two entrances, so the shell supplies the base URL and the credential | D4 |
| O3 | What if `dioxus-primitives` or `dioxus-sdk-time` has no release for Dioxus 0.8 | (a) wait for a release; (b) replace them with maintained libraries or own components | Decide on evidence at D4 start. No fork and no patch | D4 |
| O4 | How the dist workflow and the template's `release.yml` share a path | Unverified: whether dist can write its workflow under another name. If not, a template change is needed | Check the dist configuration reference at T1 | T1 |

## Units

Units run in this order. A unit starts only after its prerequisites are merged.

| Unit | Repository | Deliverable | Needs |
| T1 | momokaya-template | Project-owned extension points: Cargo workspace members, lints and licence; pnpm workspace entries; the dist release workflow beside the managed `release.yml`. A generated-copy check for each. A template release. | — |
| D1 | dogsled | Restructure: the binary to `apps/dogsled`, the Pi extension to `packages/pi-dogsled`. Every release channel builds from the new paths. No behaviour change. | — |
| D2 | dogsled | Copier baseline: generate the template with the dogsled answers into a disposable copy, reconcile it with D1, and commit `.copier-answers.yml`. | T1, D1 |
| D3 | dogsled | `dogsled-contract` from `portal-types`, and `dogsled-core` with the portal's state and the shell streams. `dogsled serve` uses the contract. | D2 |
| D4 | dogsled | The Dioxus 0.8 web shell on the core, with full portal parity. Retire `web/`. | D3, O2, O3 |
| D5 | dogsled | The gateway, sign-in and the dev stack, against a real `dogsled serve`. Update `PRODUCT.md`. | D3, O1 |
| I1 | infra | Deploy the gateway on the O1 host, bound to `tailscale0` and loopback only. | D5 |
| D6 | dogsled | iOS shell, "needs you" scope. | D3, D5 |
| D7 | dogsled | Android shell, "needs you" scope. | D3, D5 |
| D8 | dogsled | Maestro flows for web, iOS and Android. A real `copier update` to the next template release in a disposable copy. | D4, D6, D7 |

D4 and D5 can run at the same time. D6 and D7 can run at the same time. Each owns different files behind the contract.

## Acceptance

Heavy checks run before a release only. Cheap checks run on the hosts.

| Unit | Acceptance |
| T1 | `proto run moon -- run acceptance` passes on the release pull request. A generated copy keeps a project-owned workspace member, lint table and publish workflow through `copier update`. |
| D1 | `dist plan` lists the same artifacts as at `4eb6bb7`. `pnpm pack --dry-run` in `packages/pi-dogsled` lists the same files as the current package. `proto run moon -- run :#precommit` passes. |
| D2 | `.copier-answers.yml` exists and was written by Copier 9.19.1. `moon run :format :lint` passes. |
| D3 | `cargo test -p dogsled-core` covers the board, the inbox, questions, reviews and a stream that ends with an error. `dogsled serve` serves the same routes. |
| D4 | The web shell does every first-screen job in `PRODUCT.md` against a real `dogsled serve`. `web/` is gone. |
| D5 | `moon run dev`: sign in through Mailpit, then read the board and answer a question through the gateway. A request without a session fails. A request from a non-tailnet address fails. |
| I1 | From a phone on the tailnet, sign in and read the board. From outside the tailnet, the gateway does not answer. |
| D6, D7 | `moon run dev-ios` and `moon run dev-android`: sign in, see the inbox, answer a question, approve a plan review, send a message, and read host load. |
| D8 | `moon run e2e:web e2e:ios e2e:android` pass against a disposable `dogsled serve` with its own `DOGSLED_STATE`. The update brings in a template change and keeps dogsled's code, `.env.dev` and release workflows. |

E2E flows never use the owner's real `dogsled serve`. They start a disposable one with its own `DOGSLED_STATE` and
`DOGSLED_CONFIG`.

## Risks

1. Dioxus 0.8 is an alpha. Its API can change between alphas. The template pins one alpha exactly.
2. The portal rewrite (D4) is the largest unit. It moves about 12,000 lines of component state into the core.
3. The gateway makes the portal reachable from more than one machine. Sign-in and the tailnet bind are both required.
4. Release channels can break during D1. Each D1 commit must keep `dist plan` and the npm package unchanged.
