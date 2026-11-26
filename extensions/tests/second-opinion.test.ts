import { expect, test } from "bun:test";
import secondOpinion from "../second-opinion.ts";
import { withTestSession, type ProviderInput } from "./helpers.ts";

const model = (id: string) => ({ id, name: id, input: ["text" as const], reasoning: true,
	cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 10000, maxTokens: 1000 });

const provider: ProviderInput = { api: "openai-completions", baseUrl: "http://127.0.0.1:1", apiKey: "test",
	models: ["gui", "claude-review", "claude-other", "gpt-review", "us.anthropic.claude-review"].map(model) };

test("asks the first available reviewer from another family, then another model of the same family", async () => {
	const previous = process.env.LUDOLPH_REVIEWERS;
	try {
		await withTestSession([secondOpinion], async (session) => {
			const runner = session.extensionRunner;
			const review = runner.getToolDefinition("second_opinion");
			const ctx = runner.createToolContext("call", undefined);
			if (!review || !ctx.model) throw new Error("Second opinion or chat model not available");
			const claude = { ...ctx, model: { ...ctx.model, id: "claude-review" } };
			// Stopped before it starts: the reviewer it names is the choice under test.
			const stopped = AbortSignal.abort();
			const asked = async (reviewers: string[], chat = ctx): Promise<string> => {
				process.env.LUDOLPH_REVIEWERS = JSON.stringify(reviewers.map((model) => ({ model, thinking: "high" })));
				let update = "";
				const run = review.execute("call", { request: "Review the plan" }, stopped, (u) => {
					update = u.content.map((part) => part.type === "text" ? part.text : "").join("");
				}, chat);
				await expect(run).rejects.toThrow();
				return update;
			};

			const all = ["missing/gemini-review", "test/claude-review", "test/claude-other", "test/gpt-review"];
			expect(await asked(all)).toBe("asking test/claude-review…");
			expect(await asked(all, claude)).toBe("asking test/gpt-review…");
			expect(await asked(["test/claude-review", "test/claude-other"], claude)).toBe("asking test/claude-other…");
			expect(await asked(["test/us.anthropic.claude-review", "test/gpt-review"], claude)).toBe("asking test/gpt-review…");

			process.env.LUDOLPH_REVIEWERS = JSON.stringify([{ model: "test/claude-review", thinking: "high" }]);
			await expect(review.execute("call", { request: "Review the plan" }, undefined, undefined, claude))
				.rejects.toThrow("None of the reviewers (test/claude-review) is available and different from claude-review");
			delete process.env.LUDOLPH_REVIEWERS;
			await expect(review.execute("call", { request: "Review the plan" }, undefined, undefined, ctx))
				.rejects.toThrow("Choose a reviewer in Ludolph Settings");
		}, { test: provider });
	} finally {
		if (previous === undefined) delete process.env.LUDOLPH_REVIEWERS;
		else process.env.LUDOLPH_REVIEWERS = previous;
	}
});
