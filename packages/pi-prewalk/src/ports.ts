import type { ExtensionAPI, ExtensionContext, SessionEntry } from "@earendil-works/pi-coding-agent";
import type { ConfigReport } from "./wasm.ts";

/** Session authority used by the protocol adapter. */
type Host = Pick<
  ExtensionAPI,
  | "appendEntry"
  | "sendMessage"
  | "getAllTools"
  | "getActiveTools"
  | "getSettings"
  | "getThinkingLevel"
  | "setThinkingLevel"
  | "setModel"
> & { readonly events: Readonly<ExtensionAPI["events"]> };
/** Read-only pi facts. */
interface Context {
  readonly cwd: string;
  readonly isProjectTrusted: () => boolean;
  readonly sessionManager: Pick<ExtensionContext["sessionManager"], "getBranch">;
  readonly model: ExtensionContext["model"];
  readonly ui: Pick<ExtensionContext["ui"], "notify">;
  readonly modelRegistry: Pick<ExtensionContext["modelRegistry"], "getAvailable">;
}
/** Config and previous-session access, separate from runtime authority. */
interface Files {
  readonly config: (ctx: Context) => ConfigReport;
  readonly previous: (path: string) => readonly SessionEntry[];
}

export type { Context, Files, Host };
