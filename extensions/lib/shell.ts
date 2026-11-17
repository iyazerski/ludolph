import { homedir } from "node:os";
import { basename, join, resolve } from "node:path";
import { parse } from "shell-quote";

export interface ShellCommand {
	words: string[];
	operator: string;
	/** Started with `&`, alone or in a group, so the shell doesn't wait for it. */
	background: boolean;
	/** Inside a `for`, `while`, or `until` loop. */
	loop: boolean;
}

const CONTROL = ["&&", "||", ";", ";;", "|", "|&", "&", "(", ")"];
// A heredoc body is data, not commands: keep only the line that starts it.
const HEREDOC = /(<<-?\s*(['"]?)(\w+)\2[^\n]*)\n[\s\S]*?\n[\t ]*\3[\t ]*(?=\n|$)/g;

/** Tokenize simple shell commands without treating quoted code or heredocs as executable commands. */
export function shellCommands(command: string): ShellCommand[] {
	const commands: ShellCommand[] = [];
	let words: string[] = [];
	const groups: number[] = [];
	// The first command of the group that just closed, while only redirections follow it.
	let closed: number | undefined;
	let redirectTarget = false;
	let loops = 0;
	const flush = (operator: string): void => {
		if (!words.length) return;
		if (["for", "while", "until"].includes(words[0])) loops++;
		commands.push({ words, operator, background: false, loop: loops > 0 });
		if (words[0] === "done" && loops > 0) loops--;
		words = [];
	};
	try {
		// shell-quote treats bare newlines as whitespace; semicolons also preserve quoted arguments.
		const source = command.replace(HEREDOC, "$1").replace(/\\\r?\n/g, "").replace(/\r?\n/g, ";");
		for (const token of parse(source, (key) => `$${key}`)) {
			if (typeof token === "string" || ("op" in token && token.op === "glob")) {
				const word = typeof token === "string" ? token : token.pattern;
				if (!redirectTarget && !/^\d+$/.test(word)) closed = undefined;
				redirectTarget = false;
				words.push(word);
			} else if ("op" in token && CONTROL.includes(token.op)) {
				redirectTarget = false;
				if (token.op === "&" && closed !== undefined) {
					// `( ... ) >/dev/null 2>&1 &`: the redirections belong to the group.
					for (const started of commands.slice(closed)) started.background = true;
					words = [];
					closed = undefined;
					continue;
				}
				flush(token.op);
				closed = token.op === ")" ? groups.pop() : undefined;
				if (token.op === "(") groups.push(commands.length);
				if (token.op === "&") {
					// `&` starts the whole pipeline before it.
					for (let index = commands.length - 1; index >= 0; index--) {
						commands[index].background = true;
						if (!["|", "|&"].includes(commands[index - 1]?.operator ?? "")) break;
					}
				}
			} else if ("op" in token) {
				redirectTarget = true;
				words.push(token.op);
			}
		}
		flush("");
	} catch {
		// Incomplete shell syntax is not safe to classify as a command to block.
	}
	return commands;
}

/** Return the executable and arguments after shell assignments and control-flow prefixes. */
export function executable(words: string[]): string[] {
	const start = words.findIndex((word) => !/^\w+=/.test(word) && !["do", "then", "else", "!"].includes(word));
	if (start < 0) return [];
	return [basename(words[start]), ...words.slice(start + 1)];
}

/** Detect explicit file producers that must not run beside a dependent read. */
export function producesFile(command: string, file: string, cwd: string): boolean {
	const absolute = (value: string) => resolve(cwd, value.replace(/^~(?=\/|$)/, homedir()));
	for (const { words } of shellCommands(command)) {
		const [name, ...args] = executable(words);
		if (name === "cd" && args[0]) {
			cwd = absolute(args[0]);
			continue;
		}
		if (["ssh", "docker", "kubectl", "adb"].includes(name)) break;
		for (const flag of [">", ">>", "--out"]) {
			const index = args.indexOf(flag);
			if (index >= 0 && args[index + 1] && absolute(args[index + 1]) === file) return true;
		}
		if (["mv", "cp"].includes(name) && args.length >= 2) {
			const destination = args[args.length - 1];
			if (absolute(destination) === file) return true;
			if (destination.endsWith("/") && args.slice(0, -1).some((source) => absolute(join(destination, basename(source))) === file)) return true;
		}
	}
	return false;
}

/** Detect local in-place editors, leaving previews, help, and remote commands alone. */
export function shellEdits(command: string): boolean {
	for (const { words } of shellCommands(command)) {
		const [name, ...args] = executable(words);
		if (["ssh", "docker", "kubectl", "adb"].includes(name)) break;
		if (name === "sd" && !args.some((arg) => ["-h", "--help", "-V", "--version", "-p", "--preview"].includes(arg))) {
			const positional: string[] = [];
			for (let index = 0; index < args.length; index++) {
				if ([">", ">>", "<", ">&"].includes(args[index])) {
					if (/^\d+$/.test(positional.at(-1) ?? "")) positional.pop();
					index++;
				} else if (["-n", "--max-replacements", "-f", "--flags"].includes(args[index])) index++;
				else if (args[index] === "--") { positional.push(...args.slice(index + 1)); break; }
				else if (!args[index].startsWith("-")) positional.push(args[index]);
			}
			if (positional.length >= 3) return true;
		}
		if (["sed", "perl"].includes(name) && args.some((arg) => /^-[a-zA-Z]*i/.test(arg) || arg.startsWith("--in-place"))) return true;
	}
	return false;
}

/** Return whether a command runs a test, lint, type, or build check. */
function isCheck(name: string, args: string[]): boolean {
	return (name === "cargo" && ["test", "check", "clippy", "build", "fmt"].includes(args[0]))
		|| (["bun", "npm", "pnpm"].includes(name) && (args[0] === "test" || (args[0] === "run" && /^(test|check|typecheck|lint|build)(:|$)/.test(args[1] ?? ""))))
		|| (name === "prek" && args[0] === "run")
		|| ["pytest", "mypy", "pyright", "ruff"].includes(name)
		|| (name === "uv" && args[0] === "run" && ["pytest", "mypy", "pyright", "ruff"].includes(args[1]));
}

/** Return whether a command commits, pushes, or publishes a pull request. */
function publishes(name: string, args: string[]): boolean {
	if (name === "gh") return args[0] === "pr" && ["create", "merge", "ready"].includes(args[1]);
	if (name !== "git") return false;
	const subcommand = args.find((arg, index) => !arg.startsWith("-") && !["-C", "-c"].includes(args[index - 1]));
	return subcommand === "commit" || subcommand === "push";
}

/**
 * Find checks whose failure a command could miss.
 *
 * `publish`: a later commit or push in the same command runs even when a check fails, because its
 * status is hidden by a pipe filter or a `;` separates them. `hidden`: a pipe filter hides a check's
 * status, but nothing depends on it.
 */
export function checkRisk(command: string): "publish" | "hidden" | undefined {
	const capturesStatus = /\$\{?PIPESTATUS\b/.test(command);
	let pipefail = false, errexit = false, hidden = false;
	// Whether a failed check would not stop the current command: from an earlier list or this `&&` list.
	let carried = false, current = false, pending = false;
	let check = false, checkLast = false;
	// Variables that saved `$?`, and whether this list's later steps run only after testing one is 0.
	const statuses = new Set<string>();
	let cleared = false;
	// The state before a check ended with `||`, restored when what runs on failure is `exit`.
	let beforeOr: { current: boolean; pending: boolean } | undefined;
	for (const { words, operator } of shellCommands(command)) {
		const [name = "", ...args] = executable(words);
		for (const word of words) {
			const saved = /^(\w+)=\$\?$/.exec(word);
			if (saved) statuses.add(saved[1]);
		}
		if (beforeOr && name === "exit") ({ current, pending } = beforeOr);
		beforeOr = undefined;
		if (["[", "[[", "test"].includes(name) && operator === "&&" && args.some((arg) => statuses.has(arg.replace(/^\$\{?(\w+)\}?$/, "$1")))) cleared = true;
		if (name === "set") {
			for (let index = 0; index < args.length; index++) {
				const flags = /^([-+])([a-zA-Z]*)$/.exec(args[index]);
				if (!flags) continue;
				const on = flags[1] === "-";
				if (flags[2].includes("e")) errexit = on;
				if (!flags[2].endsWith("o")) continue;
				const option = args[++index];
				if (option === "pipefail") pipefail = on;
				if (option === "errexit") errexit = on;
			}
		}
		if ((carried || current) && !cleared && publishes(name, args)) return "publish";
		if (isCheck(name, args)) check = checkLast = true;
		else if (check) checkLast = false;
		if (operator === "|" || operator === "|&") continue;
		const endsList = operator !== "&&" && operator !== "||";
		if (check) {
			const visible = checkLast || pipefail || capturesStatus;
			if (visible && operator === "||") beforeOr = { current, pending };
			if (!visible) hidden = true;
			if (!visible || operator === "||") current = true;
			// `set -e` exits on a failed check only when it ends its `&&`/`||` list.
			if (!(visible && errexit && endsList)) pending = true;
		}
		check = checkLast = false;
		if (endsList) {
			carried ||= pending || current;
			current = pending = cleared = false;
		}
	}
	return hidden ? "hidden" : undefined;
}

const SECONDS: Record<string, number> = { "": 1, s: 1, m: 60, h: 3600, d: 86400 };

/** Return whether a command waits in the foreground outside a loop: a `sleep` of 10 seconds or more, or a CI watch. */
export function waitsInForeground(command: string): boolean {
	return shellCommands(command).some(({ words, background, loop }) => {
		if (background || loop) return false;
		const [name, ...args] = executable(words);
		if (name === "gh") return (args[0] === "run" && args[1] === "watch") || (args[0] === "pr" && args[1] === "checks" && args.includes("--watch"));
		const duration = name === "sleep" ? /^(\d+(?:\.\d+)?)([smhd]?)$/.exec(args[0] ?? "") : null;
		return duration !== null && Number(duration[1]) * SECONDS[duration[2]] >= 10;
	});
}
