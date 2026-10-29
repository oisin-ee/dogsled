import type { Change, Health } from "./health.ts";
import type { ExtensionContext, ModelRouteRequest } from "@earendil-works/pi-coding-agent";
import type { State } from "./policy.ts";

/** Pi authority used by routing and settlement. */
interface Context {
  readonly model: ExtensionContext["model"];
  readonly modelRegistry: Pick<ExtensionContext["modelRegistry"], "getAvailable">;
  readonly sessionManager: Pick<ExtensionContext["sessionManager"], "getBranch">;
  readonly signal?: AbortSignal | undefined;
}
/** External effects owned by the host adapter. */
interface Effects {
  readonly change: (change: Change) => Promise<void>;
  readonly now: () => number;
  readonly quota: () => Promise<string>;
  readonly read: () => Promise<Health>;
  readonly wait: (delay: number, signal?: AbortSignal) => Promise<void>;
}
/** Read-only Pi routing request. */
type Request = Readonly<
  Pick<ModelRouteRequest<State>, "model" | "reason" | "signal" | "state" | "thinkingLevel">
> & {
  readonly failed?: {
    readonly message: { readonly errorMessage?: string; readonly stopReason: string };
  };
};

export type { Context, Effects, Request };
