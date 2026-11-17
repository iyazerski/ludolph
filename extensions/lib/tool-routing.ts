import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/**
 * Tell the model which tool to use for each kind of work, in one decision list.
 *
 * A model turn takes seconds, while starting rg, jq, python, or bun takes milliseconds. So the list
 * routes work by turns and output, not by runtime: batch into codemode, read with `read`, edit with
 * `edit`. It replaces pi's own codemode and read rules, which say less and would repeat it, and the
 * bash snippet's "(ls, grep, find, etc.)", which points the model at the wrong tools.
 * tool-hints.ts nudges the model when it drifts from the list anyway.
 */

const TOOL_ROUTING = `Minimize tool turns and returned output.
- One quick call whose full output you need: call directly.
- Batch independent lookups, dependent steps, MCP/web calls, and edits with checks in \`codemode\`. Calls in one message run in parallel; chain dependent steps inside the script.
- Bound returned results. Don't dump whole files or logs.
- Search: \`rg\` for content, \`fd\` for filenames, not grep or find.
- Code navigation: when \`lspyx\` (Python) or \`gopls\` (Go) is in mcp_servers, find definitions, usages, and outlines with it in codemode, not \`rg\`.
- Read files: \`read\` with offset/limit, not cat, head, tail, or sed -n. Batch reads with parallel \`tools.read\` calls in codemode.
- JSON: \`jq\` for files and command output, \`JSON.parse\` for tool results.
- Patch existing files with \`edit\`, not shell/Python patches or read-transform-write scripts.
- Use \`write\` for new files or deliberate complete rewrites, not patches or heredocs.
- Inline scripts only for logic needing a language or library: codemode JS for tool results, otherwise \`bun -e\` or \`uv run python -\`.
- Checks: log each check to a file and save \`$?\` before filtering. Commit or push only after checks pass. In codemode, check Bash \`exit_code\`, not just \`output\`.
- Waiting: don't sleep across turns. When the next step needs a result, wait in one bounded command, such as \`timeout 600 sh -c 'until [ -s out.txt ]; do sleep 10; done'\`. For long work whose follow-ups don't need you, such as CI, start a detached watcher that logs results and runs the follow-ups, then return control. Codemode has no timers.`;

export default function (pi: ExtensionAPI) {
	pi.on("before_agent_start", (event) => {
		const options = event.systemPromptOptions;
		if (options.toolSnippets.bash) options.toolSnippets.bash = "Execute bash commands in the cwd, fresh each call (rg, fd, jq, git, etc.)";
		// The list routes work through codemode, so it only applies while codemode is available.
		if (!pi.getActiveTools().includes("codemode")) return;
		options.sections.tool_routing = TOOL_ROUTING;
		// The list covers these built-in rules.
		options.toolGuidelines.read = (options.toolGuidelines.read ?? []).filter(
			(rule) => !rule.startsWith("Use read to examine files"),
		);
		options.toolGuidelines.codemode = (options.toolGuidelines.codemode ?? []).filter(
			(rule) => !rule.startsWith("Use codemode to batch"),
		);
	});
}
