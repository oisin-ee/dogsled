// Context only. Placement and spawning remain explicit CLI/Shepherdr calls.
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function wrangle(pi: ExtensionAPI) {
	pi.on("before_agent_start", async (event) => {
		const result = await pi.exec(process.env["WRANGLE_BIN"] ?? "wrangle", ["status", "--brief"]);
		if (result.code !== 0) {
			throw new Error(`wrangle status --brief failed: ${result.stderr.trim() || `exit ${result.code}`}`);
		}
		return { systemPrompt: `${event.systemPrompt}\n\n## Host status\n${result.stdout.trim()}\n` };
	});
}
