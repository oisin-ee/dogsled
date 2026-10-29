import type {
  AfterProviderResponseEvent,
  ExtensionAPI,
  ModelRoute,
} from "@earendil-works/pi-coding-agent";
import { Store, readConfig, wait } from "./source.ts";
import type { Health } from "./health.ts";
import { Recovery } from "./recovery.ts";
import type { Request } from "./ports.ts";
import { Router } from "./router.ts";
import type { State } from "./policy.ts";
import { parseConfig } from "./config.ts";

const QUOTA_TIMEOUT = 2000,
  SUCCESS = 0,
  /**
   * Registers user-defined tiers and the final-boundary recovery adapter.
   * @param {ExtensionAPI} pi Pi extension authority.
   * @returns {Promise<void>} Registration completion.
   */
  extension = async (pi: ExtensionAPI): Promise<void> => {
    const tiers = parseConfig(await readConfig()),
      store = new Store(),
      router = new Router({
        change: async (change): Promise<void> => store.change(change),
        now: Date.now,
        quota: async (): Promise<string> => {
          try {
            const result = await pi.exec("dogsled", ["quota", "--json", "--cached"], {
              timeout: QUOTA_TIMEOUT,
            });
            return result.code === SUCCESS ? result.stdout : "";
          } catch {
            return "";
          }
        },
        read: async (): Promise<Health> => store.read(),
        wait,
      }),
      recovery = new Recovery(router, tiers);
    for (const [id, targets] of tiers) {
      pi.registerVirtualModel<State>({
        id,
        name: `Tier: ${id}`,
        provider: "tier",
        route: async (request: Request, ctx): Promise<ModelRoute<State>> =>
          router.route(targets, request, ctx),
        thinkingLevels: ["off", "minimal", "low", "medium", "high", "xhigh", "max"],
      });
    }
    pi.on(
      "after_provider_response",
      (
        event: Readonly<Omit<AfterProviderResponseEvent, "headers">> & {
          readonly headers: Readonly<Record<string, string>>;
        },
      ) => {
        router.responses.observe(event.headers, event.status);
      },
    );
    pi.on("agent_before_settle", async (event, ctx) => recovery.settle(event, ctx));
    pi.on("agent_settled", () => {
      router.responses.clear();
    });
    pi.on("session_start", () => {
      router.responses.clear();
    });
  };

export default extension;
