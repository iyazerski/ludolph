import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { resolveModel, runPi } from "./lib/child-pi.ts";

// Without extensions the child pi has no second_opinion tool, so reviews can't recurse.

const TIMEOUT_MS = 20 * 60_000;

const SETTINGS = "Ludolph Settings → Model → Second opinion";

// Reviewers tend to rate every real edge case as must-fix, so severity is tied to how likely a
// finding is and what it breaks.
const REVIEW = `Review another agent's work for bugs, edge cases, performance, and design flaws. Read files in the working directory to check details, but don't change anything.

For each finding, give:
- Severity: must-fix, should-fix, or minor.
- Where: file:line, or the part of the plan.
- Evidence: what the code does, quoting the lines that matter.
- Trigger: the inputs or conditions that cause it, and how often they happen in real use.
- Fix: the smallest change that solves it.

Rate severity by likelihood and impact, not by the worst case:
- must-fix: breaks a common workflow, or can lose data, open a security hole, or crash.
- should-fix: real, but needs uncommon conditions or has limited impact.
- minor: rare edge cases, cosmetic issues, style, and design trade-offs.
A rare edge case isn't must-fix unless its impact is severe. If you can't tell how often a trigger happens, say so rather than guess. Check each finding in the code and mark any you couldn't confirm as unverified. List findings most severe first, and don't pad the list: if nothing important is wrong, say so.`;

const params = Type.Object({
	request: Type.String({
		description: "What's in scope and what to focus on, then the content: only the diff of the work in this conversation, or the plan or design.",
	}),
});

interface Reviewer {
	model: string;
	thinking: string;
}

/** The model without its provider or Bedrock's region and vendor, such as `claude-opus-4-5` for `us.anthropic.claude-opus-4-5`. */
function modelName(id: string): string {
	return (id.split("/").at(-1) ?? id).toLowerCase().replace(/^([a-z-]+\.)+/, "");
}

function family(id: string): string {
	const name = modelName(id);
	const word = name.match(/^[a-z]+/)?.[0] ?? name;
	return word === "o" ? "gpt" : word;
}

export default function (pi: ExtensionAPI) {
	pi.registerTool({
		name: "second_opinion",
		label: "Second Opinion",
		description: [
			"Get a read-only review of a diff, plan, or design from a model of another family.",
			"Returns the reviewer's findings with evidence and severity. Check each point against the code before acting on it.",
			"Reviewers often overrate rare edge cases, so judge each point's likelihood and impact yourself.",
			"Then report the main points, which ones you agree with and how severe they are, and what you recommend.",
		].join(" "),
		parameters: params,

		async execute(_toolCallId, { request }, signal, onUpdate, ctx) {
			const reviewers: Reviewer[] = JSON.parse(process.env.LUDOLPH_REVIEWERS ?? "[]");
			if (!reviewers.length) throw new Error(`Choose a reviewer in ${SETTINGS}.`);
			const chat = ctx.model?.id ?? "";
			const others = reviewers.filter((r) => family(r.model) !== family(chat));
			const peers = reviewers.filter((r) => family(r.model) === family(chat) && modelName(r.model) !== modelName(chat));
			const picked = [...others, ...peers]
				.map((r) => ({ ...r, resolved: resolveModel(ctx, r.model) }))
				.find((r) => r.resolved);
			if (!picked?.resolved) {
				const tried = reviewers.map((r) => r.model).join(", ");
				throw new Error(`None of the reviewers (${tried}) is available and different from ${chat}. Choose another in ${SETTINGS}.`);
			}
			const model = picked.resolved;
			onUpdate?.({ content: [{ type: "text", text: `asking ${model.id}…` }], details: undefined });
			const result = await runPi({
				name: "second_opinion",
				args: [
					"--model",
					model.id,
					"--thinking",
					picked.thinking,
					"--no-session",
					"--no-extensions",
					"--no-skills",
					"--no-prompt-templates",
					"--exclude-tools",
					"edit,write",
					"--append-system-prompt",
					REVIEW,
				],
				input: request,
				cwd: ctx.cwd,
				signal,
				timeoutMs: TIMEOUT_MS,
			});
			return {
				content: [{ type: "text", text: result.text }],
				details: undefined,
				usage: result.usage,
				isError: result.failed,
			};
		},
	});
}
