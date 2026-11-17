#!/usr/bin/env bun
/**
 * Show how often agents break the tool rules at each context size, to check where they drift.
 *
 *   bun scripts/context-stats.ts [days|session-id|path.jsonl]
 *
 * Reads the sessions changed in the last `days` days (default: all) and classifies every file edit,
 * file read, and search: `edit`, `read`, `rg`, and `fd` follow the rules, while the shell commands
 * tool-hints.ts hints at (Python or sed -i edits, sed -n or cat reads) and grep or find break them.
 * Nested calls come from Pi's execution records, including dynamic Bash commands and failed calls.
 * Scripted read-transform-write patches count as edit drift; missing nested arguments are not guessed.
 * The context size of a call is the prompt size (input + cache read + cache write) of the message
 * that made it.
 *
 * It prints the break rate per context bucket, the same rates within sessions that reach 150k, the
 * rates just before and after the first compaction, and the rates split by whether the previous call
 * of the same kind broke the rule. That split matters: a rule break makes the next one much more
 * likely, so a rise with context alone can be habit, not context.
 *
 * Environment:
 *   PI_CODING_AGENT_DIR  pi's agent directory (default: ~/.pi/agent)
 */
import { classifyCall, recordedCalls, sessionFiles } from "../lib/session-calls.ts";

type Kind = "edit" | "read" | "search";

interface Call {
	kind: Kind;
	broke: boolean;
	context: number;
	session: string;
	model: string;
	/** Index of the message that made the call, within its session. */
	message: number;
	afterCompaction: boolean;
}

const KINDS: Kind[] = ["edit", "read", "search"];
const BUCKETS = [0, 25e3, 50e3, 75e3, 100e3, 125e3, 150e3, 200e3, 250e3, Number.POSITIVE_INFINITY];
// Messages on each side of the first compaction to compare.
const COMPACTION_WINDOW = 40;
/** Classify recorded calls while retaining their assistant context and compaction position. */
async function loadCalls(files: string[]): Promise<Call[]> {
	const calls: Call[] = [];
	for (const file of files) {
		for await (const call of recordedCalls(file)) {
			if (!call.context) continue;
			for (const event of classifyCall(call)) {
				calls.push({ ...event, context: call.context, session: file, model: call.model, message: call.message, afterCompaction: call.afterCompaction });
			}
		}
	}
	return calls;
}

/** Format a break rate with its sample size. */
function rate(calls: Call[]): string {
	if (calls.length === 0) return "-".padStart(6).padEnd(14);
	const broke = calls.filter((call) => call.broke).length;
	return `${((100 * broke) / calls.length).toFixed(1)}%`.padStart(6).concat(` (${calls.length})`).padEnd(14);
}

/** Format a bucket's bounds, such as 25k-50k or 250k-. */
function bucketLabel(index: number): string {
	const upper = BUCKETS[index + 1];
	return `${BUCKETS[index] / 1e3}k-${upper === Number.POSITIVE_INFINITY ? "" : `${upper / 1e3}k`}`;
}

/** Print break rates per context bucket. */
function printByContext(calls: Call[]): void {
	console.log("\nBreak rate by context size, as rate (calls):");
	console.log("context".padEnd(11), "sessions".padStart(8), ...KINDS.map((kind) => kind.padStart(6).padEnd(14)));
	for (let i = 0; i < BUCKETS.length - 1; i++) {
		const inBucket = calls.filter((call) => call.context >= BUCKETS[i] && call.context < BUCKETS[i + 1]);
		const sessions = new Set(inBucket.map((call) => call.session)).size;
		console.log(
			bucketLabel(i).padEnd(11),
			String(sessions).padStart(8),
			...KINDS.map((kind) => rate(inBucket.filter((call) => call.kind === kind))),
		);
	}
}

/** Print early and late break rates within the sessions that reach 150k. */
function printWithinSessions(calls: Call[]): void {
	const long = new Set(calls.filter((call) => call.context >= 150e3).map((call) => call.session));
	console.log(`\nSame ${long.size} sessions that reach 150k: <75k, 75k-150k, >=150k`);
	for (const kind of KINDS) {
		const own = calls.filter((call) => long.has(call.session) && call.kind === kind);
		const parts = [
			own.filter((call) => call.context < 75e3),
			own.filter((call) => call.context >= 75e3 && call.context < 150e3),
			own.filter((call) => call.context >= 150e3),
		];
		console.log(kind.padEnd(7), ...parts.map(rate));
	}
}

/** Print break rates just before and just after each session's first compaction. */
function printAroundCompaction(calls: Call[]): void {
	const compacted = [...new Set(calls.filter((call) => call.afterCompaction).map((call) => call.session))];
	console.log(`\n${compacted.length} compacted sessions: ${COMPACTION_WINDOW} messages before and after the first compaction`);
	for (const kind of KINDS) {
		const before: Call[] = [];
		const after: Call[] = [];
		for (const session of compacted) {
			const own = calls.filter((call) => call.session === session);
			const first = own.find((call) => call.afterCompaction)?.message ?? 0;
			for (const call of own) {
				if (call.kind !== kind) continue;
				if (!call.afterCompaction && call.message >= first - COMPACTION_WINDOW) before.push(call);
				else if (call.afterCompaction && call.message < first + COMPACTION_WINDOW) after.push(call);
			}
		}
		console.log(kind.padEnd(7), "before", rate(before), "after", rate(after));
	}
}

/** Print break rates split by the previous same-kind call in the session, per model with most calls. */
function printHabit(calls: Call[]): void {
	const byModel = Object.entries(Object.groupBy(calls, (call) => call.model));
	const [model, own = []] = byModel.sort((a, b) => (b[1]?.length ?? 0) - (a[1]?.length ?? 0))[0] ?? [];
	if (!model) return;
	console.log(`\n${model} only: break rate by the previous same-kind call, at <100k, 100k-175k, >=175k`);
	for (const kind of ["edit", "read"] as Kind[]) {
		const previous = new Map<string, boolean>();
		const groups = { broke: [[], [], []] as Call[][], followed: [[], [], []] as Call[][] };
		for (const call of own.filter((candidate) => candidate.kind === kind)) {
			const last = previous.get(call.session);
			previous.set(call.session, call.broke);
			if (last === undefined) continue;
			const bucket = call.context < 100e3 ? 0 : call.context < 175e3 ? 1 : 2;
			groups[last ? "broke" : "followed"][bucket].push(call);
		}
		console.log(`${kind}, previous broke`.padEnd(26), ...groups.broke.map(rate));
		console.log(`${kind}, previous followed`.padEnd(26), ...groups.followed.map(rate));
	}
}

const selector = process.argv[2] ?? "Infinity";
const calls = await loadCalls(await sessionFiles(selector));
const sessions = new Set(calls.map((call) => call.session)).size;
console.log(`${calls.length} classified calls from ${sessions} sessions (${selector})`);
printByContext(calls);
printWithinSessions(calls);
printAroundCompaction(calls);
printHabit(calls);
