#!/usr/bin/env node
/*
 * The hook command for Claude Code and Codex: `agent-hook` in their PreToolUse
 * hooks. Built to `dist/agent-hook.js`, because Node does not strip types from
 * files under `node_modules`.
 */
import { main } from "./lifecycle-runner.ts";

process.exitCode = await main();
