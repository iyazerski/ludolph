import { spawn } from "node:child_process";
import * as fs from "node:fs";
import * as path from "node:path";
import type { Message, Usage } from "@earendil-works/pi-ai";
import type { ExtensionContext } from "@earendil-works/pi-coding-agent";

const STDERR_TAIL = 2000;

export const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"] as const;
export type Thinking = typeof THINKING_LEVELS[number];

export interface ChildOptions {
	/** Names the tool in errors. */
	name: string;
	args: string[];
	/** Goes through stdin, so pi never reads a leading `-` as an option or `@path` as a file. */
	input: string;
	cwd: string;
	env?: NodeJS.ProcessEnv;
	signal?: AbortSignal;
	/** A stuck child keeps paying for tokens: stop it after this long. */
	timeoutMs: number;
	onToolStart?: (tool: string, args: unknown) => void;
}

export interface ChildResult {
	text: string;
	failed: boolean;
	usage: Usage;
}

/**
 * Resolve a model to an available `provider/model`.
 *
 * A bare model ID fails in the child when more than one logged-in provider offers it (such as
 * openai and openai-codex), so the parent picks one: the current model's provider when it offers
 * the model, else the first. Explicit provider selections never fall back to another provider.
 */
export function resolveModel(ctx: ExtensionContext, requested: string): { id: string; images: boolean } | undefined {
	const matches = ctx.modelRegistry.getAvailable().filter((model) => requested.includes("/")
		? `${model.provider}/${model.id}` === requested : model.id === requested);
	const match = matches.find((model) => model.provider === ctx.model?.provider) ?? matches[0];
	return match && { id: `${match.provider}/${match.id}`, images: match.input.includes("image") };
}

export async function runPi(options: ChildOptions): Promise<ChildResult> {
	const usage = emptyUsage();
	const messages: Message[] = [];
	let stopReason: string | undefined;
	let error: string | undefined;
	let stderr = "";
	const { command, args } = piInvocation(["--mode", "json", "-p", ...options.args]);
	const exitCode = await new Promise<number>((resolve, reject) => {
		const { signal, name } = options;
		const child = spawn(command, args, { cwd: options.cwd, env: options.env ?? process.env, stdio: ["pipe", "pipe", "pipe"] });
		// A child stopped or failed before reading its input closes the pipe: its exit reports that.
		child.stdin.on("error", () => {});
		child.stdin.end(options.input);
		let buffer = "";
		let timedOut = false;
		let parseError: Error | undefined;
		const stop = (): void => {
			child.kill("SIGTERM");
			setTimeout(() => child.kill("SIGKILL"), 5000).unref();
		};
		const timer = setTimeout(() => {
			timedOut = true;
			stop();
		}, options.timeoutMs);
		if (signal?.aborted) stop();
		else signal?.addEventListener("abort", stop, { once: true });

		const handle = (event: { type: string; [key: string]: unknown }): void => {
			if (event.type === "tool_execution_start") {
				options.onToolStart?.(event.toolName as string, event.args);
			} else if (event.type === "message_end") {
				const message = event.message as Message;
				messages.push(message);
				if (message.role === "assistant") {
					addUsage(usage, message.usage);
					stopReason = message.stopReason;
					error = message.errorMessage;
				}
			}
		};
		// JSONL records end with LF only; Unicode line separators may appear inside strings.
		child.stdout.setEncoding("utf8");
		child.stdout.on("data", (chunk: string) => {
			const lines = (buffer + chunk).split("\n");
			buffer = lines.pop() ?? "";
			for (const raw of lines) {
				const line = raw.replace(/\r$/, "");
				if (!line.trim()) continue;
				try {
					handle(JSON.parse(line));
				} catch (cause) {
					parseError = new Error(`${name} got invalid output from pi: ${String(cause)}`);
					stop();
				}
			}
		});
		child.stderr.setEncoding("utf8");
		child.stderr.on("data", (chunk: string) => {
			stderr = (stderr + chunk).slice(-STDERR_TAIL);
		});
		child.on("error", reject);
		child.on("close", (code) => {
			clearTimeout(timer);
			signal?.removeEventListener("abort", stop);
			if (parseError) reject(parseError);
			else if (signal?.aborted) reject(new Error(`${name} was aborted`));
			else if (timedOut) reject(new Error(`${name} timed out after ${options.timeoutMs / 60_000} minutes`));
			else resolve(code ?? 1);
		});
	});
	const report = finalText(messages);
	const failed = exitCode !== 0 || stopReason === "error" || stopReason === "aborted";
	const text = failed
		? `${options.name} failed (exit ${exitCode}, ${stopReason ?? "no response"}): ${error ?? (stderr.trim() || report || "no output")}`
		: report || `(${options.name} returned no report)`;
	return { text, failed, usage };
}

function piInvocation(args: string[]): { command: string; args: string[] } {
	const script = process.argv[1];
	// A compiled pi (Bun) has its entry point inside the executable.
	if (script && !script.startsWith("/$bunfs/") && fs.existsSync(script)) {
		return { command: process.execPath, args: [script, ...args] };
	}
	if (!/^(node|bun)(\.exe)?$/i.test(path.basename(process.execPath))) {
		return { command: process.execPath, args };
	}
	return { command: "pi", args };
}

function emptyUsage(): Usage {
	return {
		input: 0,
		output: 0,
		cacheRead: 0,
		cacheWrite: 0,
		totalTokens: 0,
		cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 },
	};
}

function addUsage(total: Usage, usage: Usage): void {
	total.input += usage.input;
	total.output += usage.output;
	total.cacheRead += usage.cacheRead;
	total.cacheWrite += usage.cacheWrite;
	total.totalTokens += usage.totalTokens;
	total.cost.input += usage.cost.input;
	total.cost.output += usage.cost.output;
	total.cost.cacheRead += usage.cost.cacheRead;
	total.cost.cacheWrite += usage.cost.cacheWrite;
	total.cost.total += usage.cost.total;
}

function finalText(messages: Message[]): string {
	for (let i = messages.length - 1; i >= 0; i--) {
		const message = messages[i];
		if (message.role !== "assistant") continue;
		const text = message.content
			.filter((part) => part.type === "text")
			.map((part) => part.text)
			.join("\n")
			.trim();
		if (text) return text;
	}
	return "";
}
