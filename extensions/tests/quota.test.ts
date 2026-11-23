import { expect, test } from "bun:test";
import type { ExtensionUIContext } from "@earendil-works/pi-coding-agent";
import quota, { STATUS_KEY } from "../quota.ts";
import { type ProviderInput, withTestSession } from "./helpers.ts";

const cost = { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 };
const provider = (baseUrl: string, apiKey: string, id: string): ProviderInput => ({ api: "openai-completions", baseUrl, apiKey,
	models: [{ id, name: id, input: ["text"], reasoning: true, cost, contextWindow: 10000, maxTokens: 1000 }] });

async function reports(providers: Record<string, ProviderInput>, model: [string, string], extra = [] as Parameters<typeof withTestSession>[0]) {
	const seen: { provider: string; windows: unknown[] }[] = [];
	await withTestSession([quota, ...extra], async (session) => {
		const setStatus = (key: string, text: string | undefined) => { if (key === STATUS_KEY && text) seen.push(JSON.parse(text)); };
		await session.bindExtensions({ uiContext: { setStatus } as unknown as ExtensionUIContext });
		await session.setModel(session.modelRuntime.getModel(...model)!);
		for (let i = 0; i < 100 && !seen.some(r => r.provider === model[0]); i++) await Bun.sleep(10);
	}, providers);
	return seen.filter(r => r.provider === model[0]);
}

test("reports Codex rate windows and the workspace budget", async () => {
	const claims = Buffer.from(JSON.stringify({ "https://api.openai.com/auth": { chatgpt_account_id: "acct" } })).toString("base64url");
	const asked: Headers[] = [];
	const server = Bun.serve({ port: 0, fetch(request) {
		asked.push(request.headers);
		return Response.json({
			rate_limit: {
				primary_window: { used_percent: 12, limit_window_seconds: 18000, reset_at: 1_800_000_000 },
				secondary_window: { used_percent: 40, limit_window_seconds: 604800, reset_at: 1_800_500_000 },
			},
			spend_control: { individual_limit: { unit: "usd", limit: "340.00", used: "85", used_percent: 25, reset_at: 1_801_000_000 } },
		});
	} });
	try {
		const seen = await reports({ "openai-codex": provider(server.url.href.replace(/\/$/, ""), `h.${claims}.s`, "gpt") }, ["openai-codex", "gpt"]);
		expect(seen.at(-1)?.windows).toEqual([
			{ label: "5h", used: 0.12, resetsAt: 1_800_000_000_000 },
			{ label: "Week", used: 0.4, resetsAt: 1_800_500_000_000 },
			{ label: "Budget", used: 0.25, resetsAt: 1_801_000_000_000, spent: 85, limit: 340 },
		]);
		expect(asked[0]?.get("chatgpt-account-id")).toBe("acct");
	} finally {
		server.stop(true);
	}
});

test("reports a Bifrost gateway's budgets by its host", async () => {
	const asked: { path: string; auth: string | null }[] = [];
	const server = Bun.serve({ port: 0, hostname: "127.0.0.1", fetch(request) {
		asked.push({ path: new URL(request.url).pathname, auth: request.headers.get("authorization") });
		return Response.json({ budgets: [
			{ max_limit: 200, current_usage: 50, reset_duration: "1w", last_reset: "2026-11-23T00:00:00Z" },
			{ max_limit: 600, current_usage: 60, reset_duration: "1M", last_reset: "2026-11-01T00:00:00Z" },
		] });
	} });
	try {
		// The host is how a gateway under any provider id is found.
		const baseUrl = `http://bifrost.localhost:${server.port}/anthropic`;
		const seen = await reports({ work: provider(baseUrl, "sk-bf-key", "opus") }, ["work", "opus"]);
		expect(seen.at(-1)?.windows).toEqual([
			{ label: "Week", used: 0.25, resetsAt: Date.parse("2026-11-30T00:00:00Z"), spent: 50, limit: 200 },
			{ label: "Month", used: 0.1, resetsAt: Date.parse("2026-12-01T00:00:00Z"), spent: 60, limit: 600 },
		]);
		expect(asked[0]).toEqual({ path: "/api/governance/virtual-keys/quota", auth: "Bearer sk-bf-key" });
	} finally {
		server.stop(true);
	}
});

test("reports the tightest Antigravity window among the model's groups", async () => {
	const bucket = (window: string, remainingFraction: number) => ({ window, remainingFraction, resetTime: "2026-11-30T09:08:00.000Z" });
	const groups = [
		{ displayName: "Gemini Models", buckets: [bucket("weekly", 0.9), bucket("5h", 0.7)] },
		{ displayName: "Claude and GPT models", buckets: [bucket("weekly", 0.1), bucket("5h", 0.1)] },
		{ displayName: "Gemini 4 Argon", buckets: [bucket("weekly", 0.6)] },
	];
	const antigravity: Parameters<typeof withTestSession>[0][number] = pi => {
		pi.events.on("antigravity-acp:quota:request", request => (request as { reply: (r: unknown) => void }).reply({ ok: true, quota: { at: 0, groups } }));
	};
	const seen = await reports({ "antigravity-acp": provider("http://127.0.0.1:1", "x", "gemini-4-argon") }, ["antigravity-acp", "gemini-4-argon"], [antigravity]);
	const resetsAt = Date.parse("2026-11-30T09:08:00.000Z");
	expect(seen.at(-1)?.windows).toEqual([{ label: "5h", used: 1 - 0.7, resetsAt }, { label: "Week", used: 0.4, resetsAt }]);
});
