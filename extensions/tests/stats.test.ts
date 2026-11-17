import { expect, test } from "bun:test";
import { mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { SessionManager } from "@earendil-works/pi-coding-agent";
import { toolStats } from "../scripts/tool-stats.ts";
import { assistant } from "./helpers.ts";

test("reports actual nested calls, missing records, edit recovery, and hint recurrence", async () => {
	const root = await mkdtemp(join("/tmp", "pi-stats-test-"));
	try {
		const manager = SessionManager.create(root, root);
		manager.appendMessage(assistant([
			{ type: "toolCall", id: "shell", name: "bash", arguments: { command: "sd -F old new file" } },
			{ type: "toolCall", id: "edit", name: "edit", arguments: { path: "file" } },
		]));
		manager.appendMessage({ role: "toolResult", toolCallId: "shell", toolName: "bash", content: [{ type: "text", text: "Hint [edit]: Use edit" }], isError: true, timestamp: 0 });
		manager.appendMessage({ role: "toolResult", toolCallId: "edit", toolName: "edit", content: [], isError: true, timestamp: 0 });
		manager.appendCompaction("summary", null, 1000);
		manager.appendMessage(assistant([{ type: "toolCall", id: "script", name: "codemode", arguments: { code: "if (false) tools.edit({}); const command = buildCommand(); await tools.bash({ command });" } }]));
		manager.appendMessage({ role: "toolResult", toolCallId: "script", toolName: "codemode",
			content: [{ type: "text", text: "Warning: truncated output (original token count: 9000)\nHint [edit]: Use edit" }], isError: false, timestamp: 0,
			nestedCalls: { complete: false, calls: [
				{ id: "script/1", name: "bash", arguments: { command: "sd -F old new file" }, status: "error" },
				{ id: "script/2", name: "edit", arguments: { path: "file" }, status: "ok" },
				{ id: "script/3", name: "read", arguments: { path: "file" }, status: "ok" },
				{ id: "script/4", name: "write", argumentsBytes: 100000, status: "ok" },
			] },
		});
		const file = manager.getSessionFile();
		if (!file) throw new Error("Session file not created");
		const stats = await toolStats([file]);
		expect(stats).toMatchObject({ sessions: 1, direct: 3, nested: 4, failures: 3, failed_edits: 1, recovered_edits: 1, recovery_turns: 1,
			tools: { bash: 2, edit: 2, read: 1, write: 1, codemode: 1 }, hints: { edit: 2 }, drift_after_hint: { edit: 1 },
			truncations: 1, incomplete_nested_records: 1, nested_arguments_missing: 1 });
		const child = Bun.spawn(["bun", new URL("../scripts/context-stats.ts", import.meta.url).pathname, file], { stdout: "pipe", stderr: "pipe" });
		const output = await new Response(child.stdout).text();
		expect(await child.exited, await new Response(child.stderr).text()).toBe(0);
		expect(output).toContain("5 classified calls from 1 sessions");
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});
