import { expect, test } from "bun:test";
import { join } from "node:path";
import { writeFile } from "node:fs/promises";
import type { ExtensionAPI, ToolResultEvent } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import hints, { type HintKind } from "../lib/tool-hints.ts";
import { assistant, withTestSession } from "./helpers.ts";

test("hints account switching only for failed gh auth and access commands", async () => {
	const cases: [string, string, boolean, boolean][] = [
		["gh api repos/owner/repo", "gh: Not Found (HTTP 404)", true, true],
		["gh repo view owner/repo", "GraphQL: Could not resolve to a Repository with the name 'owner/repo'. (repository)", true, true],
		["gh api user", "gh: Bad credentials (HTTP 401)", true, true],
		["gh pr create", "GraphQL: Resource not accessible by integration (createPullRequest)", true, true],
		["gh api repos/owner/repo", "gh: Forbidden (HTTP 403)", true, true],
		["gh repo clone owner/repo", "remote: Repository not found.", true, true],
		["gh api user", "HTTP 500: Internal Server Error", true, false],
		["gh api user", "HTTP 404", false, false],
		["curl --fail https://example.com", "HTTP 403", true, false],
		["ssh host gh api user", "HTTP 401", true, false],
	];
	for (const [command, output, isError, expected] of cases) {
		await withTestSession([hints], async (session) => {
			const result = await session.extensionRunner.emitToolResult({ type: "tool_result", toolName: "bash", toolCallId: "call",
				input: { command }, content: [{ type: "text", text: output }], isError, details: undefined });
			expect(JSON.stringify(result?.content ?? []).includes("Hint [github]"), command + ": " + output).toBe(expected);
		});
	}
});

test("surfaces nested gh failures on the script result", async () => {
	await withTestSession([hints], async (session) => {
		const runner = session.extensionRunner;
		await runner.emitToolResult({ type: "tool_result", toolName: "bash", toolCallId: "script/1", parentToolCallId: "script",
			input: { command: "gh repo view owner/repo" }, content: [{ type: "text", text: "gh: Not Found (HTTP 404)" }], isError: true, details: undefined });
		const result = await runner.emitToolResult({ type: "tool_result", toolName: "codemode", toolCallId: "script",
			input: { code: "" }, content: [], isError: false, details: undefined });
		expect(JSON.stringify(result?.content)).toContain("You may run `gh auth switch` and retry without asking.");
	});
});

test("guards shell edits, inline patch scripts, and commits that a failed check wouldn't stop", async () => {
	await withTestSession([hints], async (session) => {
		const runner = session.extensionRunner;
		const cases: [string, boolean][] = [
			["sd -F old new file.rs", true],
			["cd repo\nsd 'old\\nvalue' new file.rs", true],
			["sed -i 's/old/new/' file", true],
			["sd -F old new", false],
			["printf old | sd old new > /tmp/out", false],
			["sd -n 1 -F old new file", true],
			["python3 - <<'EOF'\np = 'a.rs'\ns = open(p).read()\nopen(p, 'w').write(s.replace('a', 'b'))\nEOF", true],
			["cat > /tmp/notes.sh <<'EOF'\nsed -i s/a/b/ file\nEOF", false],
			["cargo clippy 2>&1 | tail -2 && cargo test && git commit -qm fix", true],
			["set -o pipefail; cargo test 2>&1 | rg 'test result'; git commit -qm fix && git push", true],
			["set -o pipefail; set +o pipefail; uv run pytest | head && git -C repo push", true],
			["cargo test && gh pr merge 1", false],
			["cargo test > t.log 2>&1; s=$?; tail t.log; [ \"$s\" -eq 0 ] && git commit -qm fix", false],
			["cargo test > t.log 2>&1; s=$?; tail t.log; [ \"$s\" -eq 0 ] && echo ok; git push", true],
			["cargo test || exit 1; git commit -qm fix", false],
			["cargo test | tail || exit 1; git commit -qm fix", true],
			["set -euo pipefail; bun run typecheck | tail -3; git commit -qm fix", false],
			["cargo clippy 2>&1 | rg error | head -20", false],
			["for i in 1 2; do cargo test | rg 'test result'; done", false],
			["cargo test > /tmp/check.log 2>&1; status=$?; head /tmp/check.log; exit $status", false],
			["printf 'sd -F old new file.rs'", false],
			["sd --preview -F old new file", false],
			["ssh host 'sed -i s/a/b/ file'", false],
		];
		for (const [command, blocked] of cases) {
			for (const parentToolCallId of [undefined, "script"]) {
				const result = await runner.emitToolCall({ type: "tool_call", toolName: "bash", toolCallId: "call", parentToolCallId, input: { command } });
				expect(result?.block ?? false, command).toBe(blocked);
			}
		}
	});
});

test("hints at foreground waits, hidden check status, file reads, and truncated output", async () => {
	const truncation = { content: "", truncated: true, truncatedBy: "lines", totalLines: 5000, totalBytes: 1, outputLines: 2000,
		outputBytes: 1, lastLinePartial: false, firstLineExceedsLimit: false, maxLines: 2000, maxBytes: 51200 } as const;
	const bash = (command: string, kind: HintKind, expected: boolean): [ToolResultEvent, HintKind, boolean] =>
		[{ type: "tool_result", toolName: "bash", toolCallId: "call", input: { command }, content: [], isError: false, details: undefined }, kind, expected];
	const cases: [ToolResultEvent, HintKind, boolean][] = [
		bash("sleep 60; wc -c out.txt", "sleep", true),
		bash("gh run watch 1 --exit-status", "sleep", true),
		bash("(cargo test > t.log 2>&1; echo done) </dev/null >/dev/null 2>&1 & sleep 55; cat t.log", "sleep", true),
		bash("make dev > dev.log 2>&1; sleep 3; pgrep fettle", "sleep", false),
		bash("timeout 600 sh -c 'until [ -s out.txt ]; do sleep 10; done'", "sleep", false),
		bash("until [ -s out.txt ]; do sleep 10; done", "sleep", false),
		bash("( sleep 30; touch done ) >/dev/null 2>&1 &", "sleep", false),
		bash("cat > /tmp/w.sh <<'EOF'\ngh run watch 1\nEOF\nnohup sh /tmp/w.sh >/dev/null 2>&1 &", "sleep", false),
		bash("cargo test 2>&1 | tail -20", "checks", true),
		bash("sed -n 1,40p src/main.rs", "read", true),
		bash("cat docs/design.md | wc -l", "read", false),
		bash("tail -5 target/dev.log", "read", false),
		[{ type: "tool_result", toolName: "bash", toolCallId: "call", input: { command: "cargo build" }, content: [], isError: false, details: { truncation } }, "output", true],
		[{ type: "tool_result", toolName: "codemode", toolCallId: "call", input: { code: "" }, content: [{ type: "text", text: "Warning: truncated output (original token count: 9000)\n…" }], isError: false, details: undefined }, "output", true],
		[{ type: "tool_result", toolName: "read", toolCallId: "call", input: { path: "hints.ts" }, content: [{ type: "text", text: "Warning: truncated output" }], isError: false, details: undefined }, "output", false],
	];
	for (const [event, kind, expected] of cases) {
		await withTestSession([hints], async (session) => {
			const result = await session.extensionRunner.emitToolResult(event);
			expect(JSON.stringify(result?.content ?? []).includes(`Hint [${kind}]`), JSON.stringify(event.input)).toBe(expected);
		});
	}
});

test("prefers the job tool over a detached watcher or foreground CI wait", async () => {
	const jobs = (pi: ExtensionAPI) => pi.registerTool({
		name: "background_job", label: "Background job", description: "",
		parameters: Type.Object({}), execute: async () => ({ content: [], details: undefined }),
	});
	const cases: [string, boolean][] = [
		["nohup sh /tmp/watch.sh >/dev/null 2>&1 &", true],
		["gh run watch 1 --exit-status", true],
		["bun dev > /tmp/dev.log 2>&1 & sleep 2; curl --fail http://localhost:3000", false],
	];
	for (const [command, expected] of cases) {
		await withTestSession([hints, jobs], async (session) => {
			const result = await session.extensionRunner.emitToolResult({
				type: "tool_result", toolName: "bash", toolCallId: "call",
				input: { command }, content: [], isError: false, details: undefined,
			});
			expect(JSON.stringify(result?.content ?? []).includes("Use `background_job`"), command).toBe(expected);
		});
	}
});

test("points symbol searches to the connected language server", async () => {
	const lspyx = (pi: ExtensionAPI) => pi.registerTool({ name: "mcp__lspyx__explore", label: "explore", description: "",
		parameters: Type.Object({}), execute: async () => ({ content: [], details: undefined }) });
	const cases: [string, "lspyx" | "gopls", boolean][] = [
		["rg -n 'def load_settings' src", "lspyx", true],
		["cd repo && rg -n '\\bUserRepo\\b' -t py", "lspyx", true],
		["rg -n 'class Settings' backend/config.py | head", "lspyx", true],
		["rg -n 'TODO' -t py", "lspyx", false],
		["rg -n 'class Settings' src", "lspyx", false],
		["cat dump | rg 'def load'", "lspyx", false],
		["rg -n 'def load' /tmp/site-packages.txt", "lspyx", false],
		["rg -n 'func New' internal", "gopls", false],
	];
	for (const [command, kind, expected] of cases) {
		await withTestSession([hints, lspyx], async (session) => {
			const result = await session.extensionRunner.emitToolResult({ type: "tool_result", toolName: "bash", toolCallId: "call",
				input: { command }, content: [], isError: false, details: undefined });
			expect(JSON.stringify(result?.content ?? []).includes(`Hint [${kind}]`), command).toBe(expected);
		});
	}
});

test("blocks reads beside a producer and permits the next chained read", async () => {
	await withTestSession([hints], async (session, root) => {
		const runner = session.extensionRunner;
		const image = join(root, "screen.png");
		session.sessionManager.appendMessage(assistant([
			{ type: "toolCall", id: "move", name: "bash", arguments: { command: "mv screen.png " + root + "/" } },
			{ type: "toolCall", id: "read", name: "read", arguments: { path: image } },
		]));
		const event = { type: "tool_call", toolName: "read", toolCallId: "read", input: { path: image } } as const;
		expect((await runner.emitToolCall(event))?.block).toBe(true);
		session.sessionManager.appendMessage(assistant([{ type: "toolCall", id: "read", name: "read", arguments: { path: image } }]));
		expect(await runner.emitToolCall(event)).toBeUndefined();
	});
});

test("blocks scripted patches, aggregates recovery hints, and preserves structured output", async () => {
	await withTestSession([hints], async (session, root) => {
		const runner = session.extensionRunner;
		await writeFile(join(root, "file"), "old");
		const code = "const text = await tools.read({path: 'file'}); await tools.write({path: 'file', content: text.replace('old', 'new')});";
		await runner.emitToolCall({ type: "tool_call", toolName: "codemode", toolCallId: "script", input: { code } });
		const write = { type: "tool_call", toolName: "write", toolCallId: "script/2", parentToolCallId: "script", input: { path: "file", content: "new" } } as const;
		expect((await runner.emitToolCall(write))?.block).toBe(true);
		expect(await runner.emitToolCall({ ...write, input: { path: "new-file", content: "new" } })).toBeUndefined();
		expect(await runner.emitToolCall({ ...write, parentToolCallId: undefined })).toBeUndefined();
		await runner.emitToolResult({ type: "tool_result", toolName: "edit", toolCallId: "script/1", parentToolCallId: "script", input: { path: "file" }, content: [], details: undefined, isError: true });
		const result = await runner.emitToolResult({ type: "tool_result", toolName: "codemode", toolCallId: "script",
			input: { code },
			content: [], structuredContent: { ok: true }, isError: false, details: undefined });
		expect(result?.structuredContent).toEqual({ ok: true });
		expect(JSON.stringify(result?.content)).toContain("Hint [recovery]");
		expect(JSON.stringify(result?.content)).toContain("Hint [edit]");
	});
});
