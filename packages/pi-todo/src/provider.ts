import type {
  ActiveUnit,
  ClaimResult,
  ImportReceipt,
  SettlementResult,
  TodoList,
  UnitContract,
  UnitRef,
} from "./wasm.ts";
import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import { STATE_ENTRY, branchSnapshot } from "./state.ts";
import type { TodoParams, TodoResult, TodoSession } from "./tool.ts";
import { active, claim, importUnit, imported, inspect, parse, protect, settle } from "./wasm.ts";
import { ownsTodo } from "./lifecycle.ts";
import { repositoryIdentity } from "./repository.ts";
import { runTodo } from "./tool.ts";

/** Stable extension-only result envelope. */
type Result<Value> =
  | { readonly ok: true; readonly value: Value }
  | { readonly ok: false; readonly error: { readonly code: string; readonly message: string } };
/** Wire acknowledgements use explicit null for an unsealed import. */
type ImportAcknowledgement = Omit<ImportReceipt, "seal"> & {
  readonly seal: ImportReceipt["seal"] | null;
};
/** Capability acquisition does not expose a model-facing tool. */
interface TaskProviderV1 {
  readonly inspect: (id: string) => Result<UnitContract>;
  readonly claim: (input: unknown) => Result<ClaimResult>;
  readonly settle: (input: unknown) => Result<SettlementResult>;
  readonly active: () => Result<ActiveUnit | null>;
  readonly imported: () => Result<ImportAcknowledgement | null>;
}
interface ProviderHost {
  readonly appendEntry: ExtensionAPI["appendEntry"];
  readonly getAllTools: ExtensionAPI["getAllTools"];
  readonly events: Readonly<ExtensionAPI["events"]>;
}
interface Acquisition {
  readonly version: number;
  readonly reply: (provider: TaskProviderV1) => void;
}

const version = 1,
  /**
   * Validates an extension capability acquisition.
   * @param {unknown} request Untrusted event payload.
   * @returns {boolean} Whether a versioned callback is present.
   */
  acquisition = (request: unknown): request is Acquisition =>
    typeof request === "object" &&
    request !== null &&
    "version" in request &&
    request.version === version &&
    "reply" in request &&
    typeof request.reply === "function" &&
    Object.keys(request).every((key) => key === "version" || key === "reply"),
  /**
   * Runs synchronous policy through the capability envelope.
   * @param {Function} operation Core operation.
   * @returns {Result} Output or explicit failure.
   */
  result = <Value>(operation: () => Value): Result<Value> => {
    try {
      return { ok: true, value: operation() };
    } catch (error: unknown) {
      const message = error instanceof Error ? error.message : String(error),
        [code] = message.split(":");
      return { error: { code: code ?? "CONTRACT_INCOMPLETE", message }, ok: false };
    }
  };

/** One serialization boundary for todo operations and extension mutations. */
class TaskProvider {
  readonly #host: ProviderHost;
  readonly #leases = new Map<string, UnitRef>();
  #context: ExtensionContext | undefined;
  #mutating = false;
  #generation = Symbol("task-provider");

  public constructor(host: ProviderHost) {
    this.#host = host;
  }

  /** Registers a versioned synchronous capability only for the winning todo owner. */
  public register(): void {
    this.#host.events.on("oisin.tasks.acquire.v1", (request: unknown): void => {
      if (!ownsTodo(this.#host) || !acquisition(request)) {
        return;
      }
      request.reply(this.capability());
    });
  }

  /**
   * Refreshes the context at the actual call boundary.
   * @param {ExtensionContext} ctx Current runtime context.
   */
  public context(ctx: ExtensionContext): void {
    this.#context = ctx;
  }

  /** Removes runtime-only leases on extension shutdown. */
  public stop(): void {
    this.#generation = Symbol("task-provider");
    this.#leases.clear();
    this.#context = undefined;
  }

  /**
   * Holds a revision until this invocation ends.
   * @param {string} id Runtime invocation ID.
   * @param {Readonly<UnitRef>} reference Executing revision.
   */
  public lease(id: string, reference: Readonly<UnitRef>): void {
    this.#leases.set(id, structuredClone(reference));
  }

  /**
   * Releases only the matching invocation.
   * @param {string} id Ended invocation ID.
   */
  public release(id: string): void {
    this.#leases.delete(id);
  }

  /**
   * Serializes normal todo operations with extension mutations.
   * @param {TodoParams} params Model-authored request.
   * @param {ExtensionContext} ctx Current context.
   * @returns {TodoResult} Persisted result.
   */
  public run(params: TodoParams, ctx: ExtensionContext): TodoResult {
    this.context(ctx);
    return this.#mutation(() => runTodo(params, this.#session()));
  }

  /**
   * Imports startup transport through the same atomic mutation boundary.
   * @param {string} source Untrusted seed JSON.
   * @param {string} origin Checkout origin URL.
   * @param {ExtensionContext} ctx Current runtime authority.
   */
  public import(source: string, origin: string, ctx: ExtensionContext): void {
    this.context(ctx);
    this.#mutation(() => {
      const session = this.#session(),
        change = importUnit(
          branchSnapshot(session.branch()),
          source,
          session.sessionId,
          repositoryIdentity(origin),
        );
      if (change.persist) {
        session.append(STATE_ENTRY, change.list);
      }
    });
  }

  /**
   * Creates methods that recheck ownership and branch state.
   * @returns {TaskProviderV1} Extension-only capability.
   */
  public capability(): TaskProviderV1 {
    const generation = this.#generation,
      respond = <Value>(operation: () => Value): Result<Value> =>
        this.#respond(generation, operation);
    return {
      active: () =>
        respond(() => {
          const session = this.#session();
          return active(branchSnapshot(session.branch()), session.sessionId) ?? null;
        }),
      claim: (input) =>
        respond(() =>
          this.#mutation(() => {
            const session = this.#session(),
              change = claim(
                branchSnapshot(session.branch()),
                JSON.stringify(input),
                session.sessionId,
              );
            if (change.persist) {
              session.append(STATE_ENTRY, change.list);
            }
            return change.value;
          }),
        ),
      imported: () => respond(() => this.#imported()),
      inspect: (id) => respond(() => inspect(branchSnapshot(this.#session().branch()), id)),
      settle: (input) =>
        respond(() =>
          this.#mutation(() => {
            const session = this.#session(),
              change = settle(
                branchSnapshot(session.branch()),
                JSON.stringify(input),
                session.sessionId,
              );
            if (change.persist) {
              session.append(STATE_ENTRY, change.list);
            }
            return change.value;
          }),
        ),
    };
  }

  #imported(): ImportAcknowledgement | null {
    const session = this.#session(),
      receipt = imported(branchSnapshot(session.branch()), session.sessionId);
    return receipt === undefined
      ? null
      : Object.assign({}, receipt, { seal: receipt.seal ?? null });
  }

  #respond<Value>(generation: symbol, operation: () => Value): Result<Value> {
    return result(() => {
      if (generation !== this.#generation) {
        throw new Error("PROVIDER_UNAVAILABLE: capability belongs to a closed runtime");
      }
      return operation();
    });
  }

  #mutation<Value>(operation: () => Value): Value {
    if (this.#mutating) {
      throw new Error("UNIT_ALREADY_BOUND: a task mutation is already in progress");
    }
    this.#mutating = true;
    try {
      return operation();
    } finally {
      this.#mutating = false;
    }
  }

  /**
   * Rejects branch rewind around an immutable seed or seal.
   * @param {ExtensionContext} ctx Runtime session.
   * @param {TodoList} branch Current branch snapshot.
   */
  static #importScope(ctx: ExtensionContext, branch: TodoList): void {
    const sessionId = ctx.sessionManager.getSessionId(),
      receipt = imported(branch, sessionId);
    for (const entry of ctx.sessionManager.getEntries()) {
      if (entry.type === "custom" && entry.customType === STATE_ENTRY) {
        const recorded = imported(parse(entry.data), sessionId);
        if (
          recorded !== undefined &&
          (receipt === undefined ||
            JSON.stringify(recorded.execution) !== JSON.stringify(receipt.execution) ||
            recorded.digest !== receipt.digest ||
            (recorded.seal !== undefined && receipt.seal === undefined))
        ) {
          throw new Error("SEEDED_SCOPE_LOCKED: branch cannot discard imported scope or its seal");
        }
      }
    }
  }

  #session(): TodoSession {
    const ctx = this.#context;
    if (ctx === undefined || !ownsTodo(this.#host)) {
      throw new Error("PROVIDER_UNAVAILABLE: pi-todo does not own an active session");
    }
    const sessionId = ctx.sessionManager.getSessionId(),
      leaf = ctx.sessionManager.getLeafId(),
      before = branchSnapshot(ctx.sessionManager.getBranch());
    TaskProvider.#importScope(ctx, before);
    return {
      append: (customType: string, list: TodoList): void => {
        if (
          ctx.sessionManager.getSessionId() !== sessionId ||
          ctx.sessionManager.getLeafId() !== leaf
        ) {
          throw new Error("STALE_ORIGIN: session branch changed during task mutation");
        }
        protect(before, list, [...this.#leases.values()]);
        this.#host.appendEntry(customType, list);
      },
      branch: () => ctx.sessionManager.getBranch(),
      sessionId,
    };
  }
}

export { TaskProvider };
export type { Result, TaskProviderV1 };
