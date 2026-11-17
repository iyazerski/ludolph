import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { resolveModel, runPi, type Thinking } from "./lib/child-pi.ts";

/**
 * Get a read-only review from the other model family: GPT for Claude, Claude for everything else.
 *
 * The peer is a child pi without edit and write tools, extensions, skills, or a session. Without
 * extensions it has no second_opinion tool, so a review can't ask for another review.
 */

const PEERS: Record<"claude" | "gpt", { model: string; thinking: Thinking }> = {
	claude: { model: "claude-opus-5-5", thinking: "medium" },
	gpt: { model: "gpt-6.1-sol", thinking: "high" },
};
const TIMEOUT_MS = 20 * 60_000;

const REVIEW = `Review another agent's work. Give a concise review of bugs, edge cases, performance, and design flaws, most important first. Read files in the working directory to check details, but don't change anything.`;

const params = Type.Object({
	request: Type.String({
		description: "What's in scope and what to focus on, then the content: only the diff of the work in this conversation, or the plan or design.",
	}),
});

export default function (pi: ExtensionAPI) {
	pi.registerTool({
		name: "second_opinion",
		label: "Second Opinion",
		description: [
			"Get a read-only review of a diff, plan, or design from the other model family (GPT if you're Claude, otherwise Claude).",
			"Returns the peer's review. Check each point against the code before acting on it, then report the main points, which ones you agree with, and what you recommend.",
		].join(" "),
		parameters: params,

		async execute(_toolCallId, { request }, signal, onUpdate, ctx) {
			const peer = ctx.model?.id.startsWith("claude") ? PEERS.gpt : PEERS.claude;
			const model = resolveModel(ctx, peer.model);
			if (!model) throw new Error(`The second opinion needs ${peer.model}, and no logged-in provider offers it.`);
			onUpdate?.({ content: [{ type: "text", text: `asking ${model.id}…` }], details: undefined });
			const result = await runPi({
				name: "second_opinion",
				args: [
					"--model",
					model.id,
					"--thinking",
					peer.thinking,
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
				// Count the peer's tokens in this session's totals.
				usage: result.usage,
				isError: result.failed,
			};
		},
	});
}
