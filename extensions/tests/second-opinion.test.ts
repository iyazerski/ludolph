import { expect, test } from "bun:test";
import secondOpinion from "../second-opinion.ts";
import { withTestSession } from "./helpers.ts";

test("uses the opposite-family reviewer and reports when it needs replacing", async () => {
	const settings = {
		LUDOLPH_REVIEW_CLAUDE_MODEL: "test/claude-review",
		LUDOLPH_REVIEW_CLAUDE_THINKING: "medium",
		LUDOLPH_REVIEW_GPT_MODEL: "test/gpt-review",
		LUDOLPH_REVIEW_GPT_THINKING: "high",
	};
	const previous = Object.keys(settings).map((key) => [key, process.env[key]] as const);
	Object.assign(process.env, settings);
	try {
		await withTestSession([secondOpinion], async (session) => {
			const runner = session.extensionRunner;
			const review = runner.getToolDefinition("second_opinion");
			const ctx = runner.createToolContext("call", undefined);
			if (!review || !ctx.model) throw new Error("Second opinion or chat model not available");
			const input = { request: "Review the plan" };

			await expect(review.execute("call", input, undefined, undefined, ctx))
				.rejects.toThrow("test/claude-review is unavailable");

			const claude = { ...ctx, model: { ...ctx.model, id: "anthropic/claude-chat" } };
			await expect(review.execute("call", input, undefined, undefined, claude))
				.rejects.toThrow("test/gpt-review is unavailable");

			delete process.env.LUDOLPH_REVIEW_GPT_MODEL;
			await expect(review.execute("call", input, undefined, undefined, claude))
				.rejects.toThrow("Choose a GPT reviewer in Ludolph Settings");
		});
	} finally {
		for (const [key, value] of previous) {
			if (value === undefined) delete process.env[key];
			else process.env[key] = value;
		}
	}
});
