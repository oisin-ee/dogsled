/*
 * Loads the Rust core. The `wasm-bindgen --target web` loader fetches its
 * `.wasm` by URL, and Node's fetch rejects `file:` URLs, so the bytes are read
 * from disk and instantiated synchronously, as pi-mcp-adapter does
 * (`mcp-script-wasm.ts`). The extension factory is synchronous, so the core
 * must be ready before it returns. An importer of this module runs after this
 * body, so the re-exports below are always initialized.
 */
import { initSync } from "./generated/todo_core.js";
import { readFileSync } from "node:fs";

initSync({
  module: readFileSync(new URL("generated/todo_core_bg.wasm", import.meta.url)),
});

export {
  active,
  apply,
  claim,
  inspect,
  import_unit as importUnit,
  imported,
  protect,
  settle,
  completion_reminder as completionReminder,
  init,
  parse,
  sections,
  view,
  widget,
} from "./generated/todo_core.js";
export type {
  Acceptance,
  ActiveUnit,
  Assignment,
  ClaimInput,
  ClaimResult,
  ImportReceipt,
  Seed,
  EvidenceRef,
  ExecutionRef,
  Settlement,
  SettlementResult,
  TaskBinding,
  TaskExport,
  ContractDraft,
  Scope,
  MutationContext,
  Unit,
  UnitContract,
  UnitRef,
  Operation,
  ReminderFact,
  Request,
  SectionChild,
  SectionItem,
  Status,
  Subtask,
  Task,
  TodoList,
  TodoSection,
  WidgetKind,
  WidgetRow,
} from "./generated/todo_core.js";
