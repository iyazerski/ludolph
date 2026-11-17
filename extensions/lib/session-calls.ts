import { createReadStream, existsSync } from "node:fs";
import { stat } from "node:fs/promises";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { Glob } from "bun";
import type { SessionEntry, SessionHeader } from "@earendil-works/pi-coding-agent";
import { executable, shellCommands } from "./shell.ts";
import { codemodeHints, commandHints } from "./tool-hints.ts";

export interface RecordedCall {
	id: string;
	name: string;
	arguments: Record<string, unknown>;
	model: string;
	context: number;
	message: number;
	afterCompaction: boolean;
	line: number;
	source: "direct" | "nested";
	failed?: boolean;
	durationMs?: number;
	output?: string;
	incompleteNested?: boolean;
	argumentsMissing?: boolean;
}

/** Select session files by age in days, session ID, or an explicit JSONL path. */
export async function sessionFiles(selector: string, sessionsDir = `${process.env.PI_CODING_AGENT_DIR ?? `${process.env.HOME}/.pi/agent`}/sessions`): Promise<string[]> {
	if (existsSync(selector) && (await stat(selector)).isFile()) return [resolve(selector)];
	const days = Number(selector);
	const byAge = !Number.isNaN(days);
	if (byAge && days <= 0) throw new Error("Days must be greater than zero");
	if (!byAge && !/^[\w.-]+$/.test(selector)) throw new Error(`Session file does not exist: ${selector}`);
	const files: string[] = [];
	for await (const file of new Glob("**/*.jsonl").scan(sessionsDir)) {
		if (!byAge && !file.split("/").at(-1)?.includes(selector)) continue;
		const path = join(sessionsDir, file);
		if (byAge && (await stat(path)).mtimeMs < Date.now() - days * 86_400_000) continue;
		files.push(path);
	}
	if (!files.length && !byAge) throw new Error(`No session matches ${selector}`);
	return files.sort();
}

/** Stream actual direct and nested calls, joining each result to its assistant context. */
export async function* recordedCalls(path: string): AsyncGenerator<RecordedCall> {
	const pending = new Map<string, RecordedCall>();
	let afterCompaction = false;
	let message = 0;
	let line = 0;
	const reader = createInterface({ input: createReadStream(path), crlfDelay: Infinity });
	for await (const raw of reader) {
		line++;
		if (!raw.trim()) continue;
		let entry: SessionEntry | SessionHeader;
		try {
			entry = JSON.parse(raw);
		} catch {
			throw new Error(`Invalid session JSON at ${path}:${line}`);
		}
		if (entry.type === "compaction") afterCompaction = true;
		if (entry.type !== "message") continue;
		const value = entry.message;
		if (value.role === "assistant") {
			message++;
			for (const part of value.content) {
				if (part.type !== "toolCall") continue;
				pending.set(part.id, {
					id: part.id, name: part.name, arguments: part.arguments,
					model: value.model, context: value.usage.input + value.usage.cacheRead + value.usage.cacheWrite,
					message, afterCompaction, line, source: "direct",
				});
			}
		} else if (value.role === "toolResult") {
			const call = pending.get(value.toolCallId);
			if (!call) continue;
			pending.delete(value.toolCallId);
			yield {
				...call, failed: value.isError, durationMs: value.durationMs,
				output: value.content.map((part) => part.type === "text" ? part.text : "").join("\n"),
				incompleteNested: value.nestedCalls?.complete === false,
			};
			for (const nested of value.nestedCalls?.calls ?? []) {
				yield {
					...call, id: nested.id, name: nested.name, arguments: nested.arguments ?? {},
					source: "nested", failed: nested.status !== "ok", durationMs: nested.durationMs,
					output: nested.error, argumentsMissing: nested.arguments === undefined,
				};
			}
		}
	}
	for (const call of pending.values()) yield call;
}

/** Classify observed calls without reconstructing nested calls from JavaScript source. */
export function classifyCall(call: RecordedCall): { kind: "edit" | "read" | "search"; broke: boolean }[] {
	if (call.name === "edit" || call.name === "read") return [{ kind: call.name, broke: false }];
	if (call.name === "codemode" && typeof call.arguments.code === "string") {
		return codemodeHints(call.arguments.code).includes("edit") ? [{ kind: "edit", broke: true }] : [];
	}
	if (call.name !== "bash" || typeof call.arguments.command !== "string") return [];
	const events: { kind: "edit" | "read" | "search"; broke: boolean }[] = [];
	const hints = commandHints(call.arguments.command);
	if (hints.includes("edit")) events.push({ kind: "edit", broke: true });
	if (hints.includes("read")) events.push({ kind: "read", broke: true });
	for (const { words } of shellCommands(call.arguments.command)) {
		const [name] = executable(words);
		if (name === "rg" || name === "fd") events.push({ kind: "search", broke: false });
		else if (name === "grep" || name === "find") events.push({ kind: "search", broke: true });
	}
	return events;
}
