#!/usr/bin/env bun
import { codemodeHints, commandHints, symbolLookup } from "../lib/tool-hints.ts";
import { recordedCalls, sessionFiles } from "../lib/session-calls.ts";

export async function toolStats(files: string[]) {
	const tools: Record<string, number> = {};
	const violations: Record<string, number> = {};
	const hints: Record<string, number> = {};
	const driftAfterHint: Record<string, number> = {};
	let direct = 0, nested = 0, failures = 0, failedEdits = 0, recoveredEdits = 0, recoveryTurns = 0;
	let incompleteNested = 0, missingArguments = 0, truncations = 0, longBash = 0, scriptedPatches = 0;
	for (const file of files) {
		const hintedAt = new Map<string, number>();
		const failedPaths = new Map<string, number>();
		for await (const call of recordedCalls(file)) {
			tools[call.name] = (tools[call.name] ?? 0) + 1;
			if (call.source === "direct") direct++; else nested++;
			if (call.failed) failures++;
			if (call.incompleteNested) incompleteNested++;
			if (call.argumentsMissing) missingArguments++;
			if (/^Warning: truncated output \(original token count:|\[Showing (?:lines|last) [^\]]*Full output: [^\]]+\]$/m.test(call.output ?? "")) truncations++;
			if (call.name === "bash" && (call.durationMs ?? 0) > 60_000) longBash++;
			const kinds = call.name === "bash" && typeof call.arguments.command === "string"
				? [...commandHints(call.arguments.command), symbolLookup(call.arguments.command) ?? []].flat()
				: call.name === "codemode" && typeof call.arguments.code === "string" ? codemodeHints(call.arguments.code) : [];
			if (call.name === "codemode" && kinds.includes("edit")) scriptedPatches++;
			for (const kind of kinds) {
				violations[kind] = (violations[kind] ?? 0) + 1;
				if (call.message > (hintedAt.get(kind) ?? Infinity)) driftAfterHint[kind] = (driftAfterHint[kind] ?? 0) + 1;
			}
			if (call.name === "edit") {
				const path = call.arguments.path;
				if (call.failed) {
					failedEdits++;
					if (typeof path === "string" && !failedPaths.has(path)) failedPaths.set(path, call.message);
				} else if (call.failed === false && typeof path === "string") {
					const failedAt = failedPaths.get(path);
					if (failedAt !== undefined) {
						recoveredEdits++;
						recoveryTurns += call.message - failedAt;
						failedPaths.delete(path);
					}
				}
			}
			for (const hint of (call.output ?? "").matchAll(/^Hint(?: \[([\w]+)\])?: (.+)$/gm)) {
				const key = hint[1] ?? hint[2];
				hints[key] = (hints[key] ?? 0) + 1;
				if (hint[1] && !hintedAt.has(hint[1])) hintedAt.set(hint[1], call.message);
			}
		}
	}
	return {
		sessions: files.length, direct, nested, tools, violations, hints, drift_after_hint: driftAfterHint,
		failures, failed_edits: failedEdits, recovered_edits: recoveredEdits, recovery_turns: recoveryTurns,
		scripted_patches: scriptedPatches, truncations, foreground_bash_over_minute: longBash,
		incomplete_nested_records: incompleteNested, nested_arguments_missing: missingArguments,
	};
}

if (import.meta.main) {
	try {
		console.log(JSON.stringify(await toolStats(await sessionFiles(process.argv[2] ?? "7")), null, 2));
	} catch (error) {
		console.error(String(error));
		process.exitCode = 1;
	}
}
