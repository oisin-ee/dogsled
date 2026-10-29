import { initSync } from "./generated/prewalk_core.js";
import { readFileSync } from "node:fs";

initSync({ module: readFileSync(new URL("generated/prewalk_core_bg.wasm", import.meta.url)) });

export { configure, decide, parse_state, project } from "./generated/prewalk_core.js";
export type {
  Action,
  Call,
  ConfigReport,
  Decision,
  Event,
  Facts,
  Flags,
  Record,
  Selection,
  State,
  Thinking,
} from "./generated/prewalk_core.js";
