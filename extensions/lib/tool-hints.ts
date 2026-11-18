import { type ExtensionAPI, isBashToolResult } from "@earendil-works/pi-coding-agent";
import { existsSync } from "node:fs";
import { homedir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { checkRisk, producesFile, shellCommands, shellEdits, waitsInForeground } from "./shell.ts";

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
	background: "Use `background_job` for long waits or detached work. In `on_success` or `on_failure`, set `notify_user` for a user notification or `resume_chat` with the next step. Then return control.",
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
// Writes to stdout or stderr print, so they don't count.
const WRITES_FROM_SCRIPT = /\bwrite_text\(|(?<!\bsys\.std(out|err))\.write\(|\bopen\([^)]*,\s*["'][wa]|writeFileSync|Bun\.write\(/;
// A script that reads a file and changes its text edits it, unlike one that only writes new output.
const READS_FROM_SCRIPT = /\bread_text\(|\.read\(\)|readFileSync|Bun\.file\(/;
const CHANGES_TEXT = /\.replace\(|\.subn?\(|\.index\(/;
// `cat > file <<EOF`, `cat <<EOF > file`, `tee file <<EOF`, but not into /dev/null.
const HEREDOC_WRITE = /\b(cat\s*>>?\s*(?!\/dev\/)\S+\s*<<|cat\s+<<-?\s*\S+\s*>>?\s*(?!\/dev\/)\S|tee\s+(-a\s+)?(?!\/dev\/)\S+\s*<<)/;
const PARSES_JSON = /\bjson\.loads?\(|-m\s+json\.tool\b/;
// `source .venv/bin/activate`, `. venv/bin/activate`, `.venv/bin/pytest`, ...: group 1 is the path before the venv.
const USES_VENV = /(?:^|[\s;&|(])(?:(?:source|\.)\s+)?((?:[^\s;&|()'"]*\/)?)\.?venv\/bin\/\w/;
// A leading `cd dir &&` or `cd dir;`, which sets where the rest of the command runs.
const LEADING_CD = /^\s*cd\s+([^\s;&|]+)\s*(?:&&|;)/;
// Scripts that do more than reshape JSON, where jq is no replacement.
const DOES_MORE = /\b(requests|urllib|http|subprocess|sqlite3|import\s+(?!json\b|sys\b)\w+)/;
// A command that reads a file, first in its pipeline: a file argument, not stdin.
const READS_FILE = /^(cat\s+(?!<<)[^\s|<>-]|head\s|tail\s(?!(.*\s)?(-[a-zA-Z]*[fF]|--follow))|sed\s+-n\s|nl\s)/;
// Commands that run on another machine, where `read`, `edit`, and jq may not reach.
const REMOTE = /\b(ssh|docker\s+(exec|run)|kubectl\s+exec|adb\s+shell)\b/;
// Loops and conditionals, where a file read is one step of shell logic that `read` can't replace.
const CONTROL_FLOW = /\b(while|until|for|then|else)\b/;
// Commands that only look things up, for counting turns of lookups in a row.
const LOOKUP_COMMANDS = new Set([
	"rg", "fd", "ls", "cat", "head", "tail", "sed", "nl", "wc", "jq", "tree", "pwd", "echo", "printf", "sort",
	"uniq", "cut", "tr", "awk", "file", "stat", "which", "command", "grep", "find", "du", "basename", "dirname",
	"readlink", "realpath", "cd", "true", "diff",
]);
// A content search, first in its pipeline: a search of piped text is no workspace lookup.
const SEARCH = /^(?:rg|grep|git\s+(?:-C\s+\S+\s+)?grep)\s/;
// Text dumps and scratch clones, which the session's language server doesn't index.
const OUTSIDE_WORKSPACE = /(?:^|\s)\/(?:tmp|private)\/|\.txt\b/;
const PYTHON_SCOPE = /\s(?:-t\s*|--type[= ])py\b|\.py\b/;
const GO_SCOPE = /\s(?:-t\s*|--type[= ])go\b|\.go\b/;
// Whitespace in a search pattern, literal or as `\s`.
const SPACE = String.raw`(?:\s|\\s)`;
const PYTHON_DEF = new RegExp(String.raw`\bdef${SPACE}`);
const PYTHON_DEFINITION = new RegExp(String.raw`\b(?:def|class)${SPACE}`);
const GO_DEFINITION = new RegExp(String.raw`\bfunc${SPACE}|\btype${SPACE}+\w+${SPACE}+(?:struct|interface)\b`);
// A whole-word search, which looks for an identifier's usages.
const WORD_SEARCH = /\s-\w*w\b|--word-regexp|\\b\w+\\b/;
const LOOKUP_GIT = /^git\s+(-C\s+\S+\s+)?(status|log|diff|show|blame|grep|ls-files|rev-parse|branch|remote)\b/;
// Lookups in a row before the batch hint: the third one gets it.
const BATCH_AFTER_TURNS = 2;
// Direct tool results to wait before repeating a hint: repeating it on every call adds noise, not habits.
const HINT_COOLDOWN = 10;

/** Split a command into its simple commands, at `;`, `&&`, `||`, newlines, and optionally pipes. */
export function segments(command: string, pipes: boolean): string[] {
	return command
		.split(pipes ? /&&|\|\||[;\n|]/ : /&&|\|\||[;\n]/)
		.map((part) => part.trim().replace(/^(\w+=\S*\s+)+/, ""))
		.filter(Boolean);
}

/** Return whether a command only looks things up and changes nothing. */
function isLookup(command: string): boolean {
	const unredirected = command.replace(/\d?>&\d|\d?>\s*\/dev\/null/g, "");
	if (/>|<</.test(unredirected) || shellEdits(command) || RUNS_SCRIPT.test(command)) return false;
	return segments(unredirected, true).every(
		(part) => LOOKUP_COMMANDS.has(part.split(/\s/)[0]) || LOOKUP_GIT.test(part),
	);
}

/** Return the part of a command before its first remote command, which is the only part that runs here. */
function localPart(command: string): string {
	const remote = command.search(REMOTE);
	return remote < 0 ? command : command.slice(0, remote);
}

/** Resolve a shell path against a directory, expanding a leading `~`. */
function resolvePath(base: string, path: string): string {
	return resolve(base, path.replace(/^~(?=\/|$)/, homedir()));
}

/** Return whether a directory or one of its parents has a `uv.lock`. */
function inUvProject(dir: string): boolean {
	for (let current = dir; ; current = dirname(current)) {
		if (existsSync(join(current, "uv.lock"))) return true;
		if (dirname(current) === current) return false;
	}
}

/** Return whether a command activates or calls a venv of a uv project, which `uv run` replaces. */
export function usesVenvInUvProject(command: string, cwd: string): boolean {
	const local = localPart(command);
	const venv = USES_VENV.exec(local);
	if (!venv) return false;
	const cd = LEADING_CD.exec(local);
	const base = cd ? resolvePath(cwd, cd[1]) : cwd;
	return inUvProject(resolvePath(base, venv[1] || "."));
}

/** Return whether a local inline script reads a file, changes its text, and writes it back. */
function rewritesFile(local: string): boolean {
	return RUNS_SCRIPT.test(local) && WRITES_FROM_SCRIPT.test(local) && READS_FROM_SCRIPT.test(local) && CHANGES_TEXT.test(local);
}

/** Return whether a simple command shows a file's text, not a count or a log tail. */
function readsIntoContext(part: string): boolean {
	const [first, next] = part.split("|").map((piece) => piece.trim());
	if (!READS_FILE.test(first) || /\.log\b/.test(first)) return false;
	return next === undefined || /^(head|tail|sed|nl|cat)\b/.test(next);
}

/** Return the hints a bash command deserves, apart from the batch and uv hints. */
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

/** Identify likely scripted patches for hints and guarded existing-file writes. */
export function codemodeHints(code: string): HintKind[] {
	const writes = /tools\.write\s*\(/.test(code);
	const reads = /tools\.read\s*\(|tools\.bash\s*\(\{\s*command:\s*[`"']cat\s/.test(code);
	return writes && reads && /\.(replace(?:All)?|slice|splice|indexOf)\s*\(/.test(code) ? ["edit"] : [];
}

/** Render hints as the text block appended to a tool result. */
function hintText(hints: Iterable<HintKind>): string {
	return [...hints].map((kind) => `Hint [${kind}]: ${HINTS[kind]}`).join("\n");
}

export default function (pi: ExtensionAPI) {
	// Hints from calls inside a codemode script, keyed by the script's tool call.
	const nested = new Map<string, Set<HintKind>>();
	const scriptedPatches = new Set<string>();
	// Turns in a row whose direct calls were all lookups in bash.
	let lookupTurns = 0;
	// What this turn's direct calls were: none yet, only bash lookups, or anything else.
	let turn: "none" | "lookup" | "other" = "none";
	// Names of this turn's direct calls, which all come from one message and run in parallel.
	const turnCalls = new Map<string, string>();
	// Direct tool results so far, and the one each hint was last shown on.
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
			if (pi.getActiveTools().includes("background_job")
				&& (hints.has("sleep") || shellCommands(localPart(command)).some(({ background }) => background))) {
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
		// A read that fails on a missing file, next to a call that may be creating it.
		const siblings = [...turnCalls].filter(([id, name]) => id !== event.toolCallId && name !== "read");
		if (event.toolName === "read" && !event.parentToolCallId && event.isError && siblings.length > 0) {
			const text = event.content.map((part) => (part.type === "text" ? part.text : "")).join("");
			if (text.includes("ENOENT")) hints.add("parallel");
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
