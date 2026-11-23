import { type ExtensionAPI, isBashToolResult } from "@earendil-works/pi-coding-agent";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { checkRisk, executable, producesFile, shellCommands, shellEdits, waitsInForeground } from "./shell.ts";

/**
 * Add a one-line hint to a tool result when the call drifts from tool-routing.ts's list.
 *
 * Rules in the system prompt alone didn't stop the habits: sessions still read files with sed -n and
 * edited them with Python scripts. A hint in the tool result lands right where the model decides its
 * next call, costs no extra turn (blocking the call would), and leaves the cached prompt alone.
 * Hints for calls made inside a codemode script are added to the script's result, since the script,
 * not the model, receives the nested result.
 */

export type HintKind = "edit" | "write" | "read" | "jq" | "sleep" | "background" | "batch" | "parallel" | "uv" | "checks" | "recovery" | "output" | "github" | "lspyx" | "gopls";

const HINTS: Record<HintKind, string> = {
	github: "This may be the wrong GitHub account. You may run `gh auth switch` and retry without asking.",
	edit: "Patch existing files with `edit` (`tools.edit` in codemode), not shell editors or read-transform-write scripts, which fail silently when the text doesn't match.",
	write: "Create files with the `write` tool, not heredocs.",
	read: "Read files with the `read` tool (offset/limit), or parallel `tools.read` calls in codemode.",
	jq: "Use `jq` to read JSON, not a Python script.",
	sleep: "Don't sleep across turns. Keep short prerequisite waits bounded in one command.",
	background: "Use `background_job` for long waits or detached work.",
	parallel: "Calls in one message run in parallel. Chain dependent steps in one codemode script.",
	batch: "Batch the next independent lookups in one codemode script.",
	checks: "A pipe filter hides the check's exit code. Log the check to a file, save `$?`, then filter the log.",
	recovery: "Reread the failed region. Retry `edit` with exact, unique, non-overlapping `oldText`; don't rewrite the whole file.",
	output: "Bound output in the command or script (filters, `head`, `rg -m`) before returning it.",
	uv: "Use `uv run <tool>`, not venv activation or `.venv/bin/`. Each bash call starts fresh.",
	lspyx: "For Python symbols, call `tools.mcp__lspyx__explore` in codemode: `query` + `workspace` finds definitions, `file` outlines a file, and `file` + `line` + `column` lists usages. Keep `rg` for plain text.",
	gopls: "For Go symbols, use gopls in codemode: `tools.mcp__gopls__go_search` finds definitions, `go_symbol_references` lists usages, and `go_package_api` shows a package's API. Keep `rg` for plain text.",
};
const CHECKS_BEFORE_PUBLISH = "A failed check would not stop the commit or push in this command. Log each check to a file and save `$?`, then commit or push only after the checks pass, chained with `&&` or in a later call.";

const RUNS_SCRIPT = /\b(python3?|bun\s+-e|node\s+-e)\b/;
const WRITES_FROM_SCRIPT = /\bwrite_text\(|(?<!\bsys\.std(out|err))\.write\(|\bopen\([^)]*,\s*["'][wa]|writeFileSync|Bun\.write\(/;
const READS_FROM_SCRIPT = /\bread_text\(|\.read\(\)|readFileSync|Bun\.file\(/;
const CHANGES_TEXT = /\.replace\(|\.subn?\(|\.index\(/;
const HEREDOC_WRITE = /\b(cat\s*>>?\s*(?!\/dev\/)\S+\s*<<|cat\s+<<-?\s*\S+\s*>>?\s*(?!\/dev\/)\S|tee\s+(-a\s+)?(?!\/dev\/)\S+\s*<<)/;
const PARSES_JSON = /\bjson\.loads?\(|-m\s+json\.tool\b/;
const USES_VENV = /(?:^|[\s;&|(])(?:(?:source|\.)\s+)?((?:[^\s;&|()'"]*\/)?)\.?venv\/bin\/\w/;
const LEADING_CD = /^\s*cd\s+([^\s;&|]+)\s*(?:&&|;)/;
const DOES_MORE = /\b(requests|urllib|http|subprocess|sqlite3|import\s+(?!json\b|sys\b)\w+)/;
const READS_FILE = /^(cat\s+(?!<<)[^\s|<>-]|head\s|tail\s(?!(.*\s)?(-[a-zA-Z]*[fF]|--follow))|sed\s+-n\s|nl\s)/;
const REMOTE = /\b(ssh|docker\s+(exec|run)|kubectl\s+exec|adb\s+shell)\b/;
const CONTROL_FLOW = /\b(while|until|for|then|else)\b/;
const LOOKUP_COMMANDS = new Set([
	"rg", "fd", "ls", "cat", "head", "tail", "sed", "nl", "wc", "jq", "tree", "pwd", "echo", "printf", "sort",
	"uniq", "cut", "tr", "awk", "file", "stat", "which", "command", "grep", "find", "du", "basename", "dirname",
	"readlink", "realpath", "cd", "true", "diff",
]);
const SEARCH = /^(?:rg|grep|git\s+(?:-C\s+\S+\s+)?grep)\s/;
// Text dumps and scratch clones aren't indexed by the session's language server.
const OUTSIDE_WORKSPACE = /(?:^|\s)\/(?:tmp|private)\/|\.txt\b/;
const PYTHON_SCOPE = /\s(?:-t\s*|--type[= ])py\b|\.py\b/;
const GO_SCOPE = /\s(?:-t\s*|--type[= ])go\b|\.go\b/;
const SPACE = String.raw`(?:\s|\\s)`;
const PYTHON_DEF = new RegExp(String.raw`\bdef${SPACE}`);
const PYTHON_DEFINITION = new RegExp(String.raw`\b(?:def|class)${SPACE}`);
const GO_DEFINITION = new RegExp(String.raw`\bfunc${SPACE}|\btype${SPACE}+\w+${SPACE}+(?:struct|interface)\b`);
const WORD_SEARCH = /\s-\w*w\b|--word-regexp|\\b\w+\\b/;
const LOOKUP_GIT = /^git\s+(-C\s+\S+\s+)?(status|log|diff|show|blame|grep|ls-files|rev-parse|branch|remote)\b/;
const BATCH_AFTER_TURNS = 2;
// Direct tool results to wait before repeating a hint: repeating it on every call adds noise, not habits.
const HINT_COOLDOWN = 10;

export function segments(command: string, pipes: boolean): string[] {
	return command
		.split(pipes ? /&&|\|\||[;\n|]/ : /&&|\|\||[;\n]/)
		.map((part) => part.trim().replace(/^(\w+=\S*\s+)+/, ""))
		.filter(Boolean);
}

function isLookup(command: string): boolean {
	const unredirected = command.replace(/\d?>&\d|\d?>\s*\/dev\/null/g, "");
	if (/>|<</.test(unredirected) || shellEdits(command) || RUNS_SCRIPT.test(command)) return false;
	return segments(unredirected, true).every(
		(part) => LOOKUP_COMMANDS.has(part.split(/\s/)[0]) || LOOKUP_GIT.test(part),
	);
}

function localPart(command: string): string {
	const remote = command.search(REMOTE);
	return remote < 0 ? command : command.slice(0, remote);
}

function resolvePath(base: string, path: string): string {
	return resolve(base, path.replace(/^~(?=\/|$)/, homedir()));
}

function inUvProject(dir: string): boolean {
	for (let current = dir; ; current = dirname(current)) {
		if (existsSync(join(current, "uv.lock"))) return true;
		if (dirname(current) === current) return false;
	}
}

export function usesVenvInUvProject(command: string, cwd: string): boolean {
	const local = localPart(command);
	const venv = USES_VENV.exec(local);
	if (!venv) return false;
	const cd = LEADING_CD.exec(local);
	const base = cd ? resolvePath(cwd, cd[1]) : cwd;
	return inUvProject(resolvePath(base, venv[1] || "."));
}

function rewritesFile(local: string): boolean {
	return RUNS_SCRIPT.test(local) && WRITES_FROM_SCRIPT.test(local) && READS_FROM_SCRIPT.test(local) && CHANGES_TEXT.test(local);
}

function readsIntoContext(part: string): boolean {
	const [first, next] = part.split("|").map((piece) => piece.trim());
	if (!READS_FILE.test(first) || /\.log\b/.test(first)) return false;
	return next === undefined || /^(head|tail|sed|nl|cat)\b/.test(next);
}

export function commandHints(command: string): HintKind[] {
	const hints: HintKind[] = [];
	if (waitsInForeground(command)) hints.push("sleep");
	if (checkRisk(command)) hints.push("checks");
	const local = localPart(command);
	const script = RUNS_SCRIPT.test(local);
	const writes = script && WRITES_FROM_SCRIPT.test(local);
	if (rewritesFile(local) || shellEdits(local)) hints.push("edit");
	else if (HEREDOC_WRITE.test(local)) hints.push("write");
	if (script && !writes && PARSES_JSON.test(local) && !DOES_MORE.test(local)) hints.push("jq");
	if (!CONTROL_FLOW.test(local) && segments(local, false).some(readsIntoContext)) hints.push("read");
	return hints;
}

/**
 * Return the language server that answers a search for a Python or Go symbol's definition or usages.
 * `class` and whole-word searches count only when scoped to the language's files, since other
 * languages share them. `def` and Go's `func` or `type T struct` mark the language on their own.
 */
export function symbolLookup(command: string): "lspyx" | "gopls" | undefined {
	const local = localPart(command);
	if (OUTSIDE_WORKSPACE.test(local)) return undefined;
	for (const part of segments(local, false)) {
		if (!SEARCH.test(part)) continue;
		const python = PYTHON_SCOPE.test(part), go = GO_SCOPE.test(part);
		if (go ? GO_DEFINITION.test(part) || WORD_SEARCH.test(part) : !python && GO_DEFINITION.test(part)) return "gopls";
		if (python ? PYTHON_DEFINITION.test(part) || WORD_SEARCH.test(part) : PYTHON_DEF.test(part)) return "lspyx";
	}
}

export function codemodeHints(code: string): HintKind[] {
	const writes = /tools\.write\s*\(/.test(code);
	const reads = /tools\.read\s*\(|tools\.bash\s*\(\{\s*command:\s*[`"']cat\s/.test(code);
	return writes && reads && /\.(replace(?:All)?|slice|splice|indexOf)\s*\(/.test(code) ? ["edit"] : [];
}

function hintText(hints: Iterable<HintKind>): string {
	return [...hints].map((kind) => `Hint [${kind}]: ${HINTS[kind]}`).join("\n");
}

export default function (pi: ExtensionAPI) {
	const nested = new Map<string, Set<HintKind>>();
	const scriptedPatches = new Set<string>();
	let lookupTurns = 0;
	let turn: "none" | "lookup" | "other" = "none";
	const turnCalls = new Map<string, string>();
	let results = 0;
	const shown = new Map<HintKind, { result: number; count: number }>();

	pi.on("agent_start", () => {
		nested.clear();
		scriptedPatches.clear();
		lookupTurns = 0;
	});
	pi.on("turn_start", () => {
		turn = "none";
		turnCalls.clear();
	});
	pi.on("tool_call", (event, ctx) => {
		if (!event.parentToolCallId) turnCalls.set(event.toolCallId, event.toolName);
		if (event.toolName === "codemode" && typeof event.input.code === "string" && codemodeHints(event.input.code).includes("edit")) scriptedPatches.add(event.toolCallId);
		if (event.toolName === "write" && event.parentToolCallId && scriptedPatches.has(event.parentToolCallId)
			&& typeof event.input.path === "string" && existsSync(resolvePath(ctx.cwd, event.input.path.replace(/^@/, "")))
			&& pi.getActiveTools().includes("edit")) return { block: true, reason: HINTS.edit };
		if (event.toolName === "bash" && typeof event.input.command === "string") {
			const command = event.input.command;
			if (pi.getActiveTools().includes("edit") && (shellEdits(command) || rewritesFile(localPart(command)))) return { block: true, reason: HINTS.edit };
			if (checkRisk(command) === "publish") return { block: true, reason: CHECKS_BEFORE_PUBLISH };
		}
		if (event.toolName !== "read" || event.parentToolCallId || typeof event.input.path !== "string") return;
		const file = resolvePath(ctx.cwd, event.input.path.replace(/^@/, ""));
		const leaf = ctx.sessionManager.getLeafEntry();
		if (leaf?.type !== "message" || leaf.message.role !== "assistant") return;
		const producer = leaf.message.content.some((part) => {
			if (part.type !== "toolCall" || part.id === event.toolCallId) return false;
			if (["edit", "write"].includes(part.name) && typeof part.arguments.path === "string") {
				return resolvePath(ctx.cwd, part.arguments.path.replace(/^@/, "")) === file;
			}
			return part.name === "bash" && typeof part.arguments.command === "string" && producesFile(part.arguments.command, file, ctx.cwd);
		});
		if (producer) return { block: true, reason: HINTS.parallel };
	});
	pi.on("turn_end", () => {
		if (turn === "lookup") lookupTurns++;
		else if (turn === "other") lookupTurns = 0;
	});

	pi.on("tool_result", (event, ctx) => {
		scriptedPatches.delete(event.toolCallId);
		const hints = new Set<HintKind>();
		const command = event.toolName === "bash" && typeof event.input.command === "string" ? event.input.command : "";
		if (command) {
			for (const kind of commandHints(command)) hints.add(kind);
			if (pi.getActiveTools().includes("background_job") && (hints.has("sleep")
				|| shellCommands(localPart(command)).some(({ words }) => ["nohup", "disown", "setsid"].includes(executable(words)[0])))) {
				hints.delete("sleep");
				hints.add("background");
			}
		}
		const server = command ? symbolLookup(command) : undefined;
		// The server's tools are listed once it connects, well before most searches.
		if (server && pi.getAllTools().some((tool) => tool.name.startsWith(`mcp__${server}__`))) hints.add(server);
		if (event.toolName === "codemode" && typeof event.input.code === "string") {
			for (const kind of codemodeHints(event.input.code)) hints.add(kind);
		}
		if (event.toolName === "edit" && event.isError) hints.add("recovery");
		const output = event.content.map((part) => part.type === "text" ? part.text : "").join("\n");
		if (event.isError && /\bgh\s/.test(localPart(command))
			&& /HTTP (?:401|403|404)\b|Could not resolve to a Repository|Resource not accessible|Bad credentials|authentication failed|repository[^\n]*not found/i.test(output)) hints.add("github");
		if ((isBashToolResult(event) && event.details?.truncation)
			|| (event.toolName === "codemode" && output.startsWith("Warning: truncated output (original token count:"))) hints.add("output");
		if (command && usesVenvInUvProject(command, ctx.cwd)) hints.add("uv");
		const siblings = [...turnCalls].filter(([id, name]) => id !== event.toolCallId && name !== "read");
		if (event.toolName === "read" && !event.parentToolCallId && event.isError && siblings.length > 0 && output.includes("ENOENT")) {
			hints.add("parallel");
		}

		if (event.parentToolCallId) {
			if (hints.size === 0) return;
			const pending = nested.get(event.parentToolCallId) ?? new Set<HintKind>();
			for (const kind of hints) pending.add(kind);
			nested.set(event.parentToolCallId, pending);
			return;
		}

		for (const kind of nested.get(event.toolCallId) ?? []) hints.add(kind);
		nested.delete(event.toolCallId);
		results++;

		const lookup = command !== "" && isLookup(command);
		if (!lookup) turn = "other";
		else if (turn === "none") {
			turn = "lookup";
			if (lookupTurns >= BATCH_AFTER_TURNS && pi.getActiveTools().includes("codemode")) {
				hints.add("batch");
				lookupTurns = 0;
			}
		}

		for (const kind of hints) {
			const last = shown.get(kind);
			const cooldown = HINT_COOLDOWN * 2 ** Math.min((last?.count ?? 1) - 1, 3);
			if (last && results - last.result < cooldown && kind !== "recovery") hints.delete(kind);
			else shown.set(kind, { result: results, count: (last?.count ?? 0) + 1 });
		}
		if (hints.size === 0) return;
		return {
			content: [...event.content, { type: "text", text: hintText(hints) }],
			// Keep the machine-readable result, which replacing `content` alone would drop.
			structuredContent: event.structuredContent,
		};
	});
}
