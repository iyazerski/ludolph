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
- Bound returned results. Don't dump whole files or logs; in \`codemode\`, slice large reads with \`offset\`/\`limit\` or filter before returning. Output over ~10k tokens (~3 source files) loses its middle.
- Search: \`rg\` for content, \`fd\` for filenames, not grep or find.
- Code navigation: when \`lspyx\` (Python) or \`gopls\` (Go) is in mcp_servers, find definitions, usages, and outlines with it in codemode, not \`rg\`.
- Read files: \`read\` with offset/limit, not cat, head, tail, or sed -n. Batch reads with parallel \`tools.read\` calls in codemode.
- JSON: \`jq\` for files and command output, \`JSON.parse\` for tool results.
- Patch existing files with \`edit\`, not shell/Python patches or read-transform-write scripts.
- Use \`write\` for new files or deliberate complete rewrites, not patches or heredocs.
- One-off scripts only when a language or library is needed: codemode JS for tool results, \`bun\` by default, \`uv run\` Python (\`--no-project --with <pkg>\` standalone, \`--no-sync\` in Python repos) for Python libs/imports. Keep inline scripts under ~10 lines; write longer ones to \`/tmp/*.{ts,py}\` and run.
- Checks: log each check to a file and save \`$?\` before filtering. Commit or push only after checks pass. In codemode, check Bash \`exit_code\`, not just \`output\`.
- Waiting: keep short prerequisite waits bounded in one command. Don't sleep across turns. Codemode has no timers.`;

const BACKGROUND = "- Long waits: use `background_job` for CI watches (`gh pr checks --watch`, `gh run watch`), polling loops, or detached work. Put follow-up steps in `on_success`/`on_failure` (`command` or `resume_chat`) and return control instead of blocking in Bash.";

export default function (pi: ExtensionAPI) {
	pi.on("before_agent_start", (event) => {
		const options = event.systemPromptOptions;
		if (options.toolSnippets.bash) options.toolSnippets.bash = "Execute bash commands in the cwd, fresh each call (rg, fd, jq, git, etc.)";
		// The list routes work through codemode, so it only applies while codemode is available.
		if (!pi.getActiveTools().includes("codemode")) return;
		options.sections.tool_routing = pi.getActiveTools().includes("background_job") ? `${TOOL_ROUTING}\n${BACKGROUND}` : TOOL_ROUTING;
		// The list covers these built-in rules.
		options.toolGuidelines.read = (options.toolGuidelines.read ?? []).filter(
			(rule) => !rule.startsWith("Use read to examine files"),
		);
		options.toolGuidelines.write = (options.toolGuidelines.write ?? []).filter(
			(rule) => !rule.startsWith("Use write only for"),
		);
		options.toolGuidelines.codemode = (options.toolGuidelines.codemode ?? []).filter(
			(rule) => !rule.startsWith("Use codemode to batch"),
		);
	});
}
