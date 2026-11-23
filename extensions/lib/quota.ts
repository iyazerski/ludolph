import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { Api, Model } from "@earendil-works/pi-ai";

// Under pi-setup's 30 s, so one slow provider leaves time for the answer.
const TIMEOUT_MS = 20_000;

export interface QuotaWindow {
	label: string;
	/** Share used, 0 to 1. */
	used: number;
	/** Epoch milliseconds. */
	resetsAt?: number;
	/** Dollars, for a budget. */
	spent?: number;
	limit?: number;
}

type Registry = ExtensionContext["modelRegistry"];

/** `account` asks for every limit of the account, not only those `model` counts against. */
type Reader = (pi: ExtensionAPI, registry: Registry, model: Model<Api>, account: boolean) => Promise<QuotaWindow[]>;

const readers: Record<string, Reader> = { "openai-codex": codex, "antigravity-acp": antigravity };

/** Gateways have no fixed provider id, so they're known by their name in the id or host. */
function readerOf(model: Model<Api>): Reader | undefined {
	if (readers[model.provider]) return readers[model.provider];
	const names = `${model.provider} ${URL.parse(model.baseUrl)?.hostname ?? ""}`.toLowerCase();
	return names.includes("bifrost") ? bifrost : undefined;
}

export function hasQuota(model: Model<Api>): boolean {
	return readerOf(model) !== undefined;
}

/** The limits `model` counts against. */
export function modelQuota(pi: ExtensionAPI, registry: Registry, model: Model<Api>): Promise<QuotaWindow[]> {
	const read = readerOf(model);
	return read ? read(pi, registry, model, false) : Promise.resolve([]);
}

/** Every account's limits by provider. A provider whose read fails is left out. */
export async function accountQuota(pi: ExtensionAPI, registry: Registry): Promise<Record<string, QuotaWindow[]>> {
	const models = new Map<string, Model<Api>>();
	for (const model of registry.getAll()) if (!models.has(model.provider) && hasQuota(model)) models.set(model.provider, model);
	const read = await Promise.allSettled([...models].map(async ([provider, model]) => [provider, await readerOf(model)!(pi, registry, model, true)] as const));
	return Object.fromEntries(read.flatMap(result => result.status === "fulfilled" ? [result.value] : []));
}

/** The usage endpoint behind Codex CLI's `/status`. */
async function codex(_pi: ExtensionAPI, registry: Registry, model: Model<Api>): Promise<QuotaWindow[]> {
	const auth = await registry.getApiKeyAndHeaders(model);
	if (!auth.ok || !auth.apiKey) throw new Error("no Codex credentials");
	const account = accountOf(auth.apiKey);
	const response = await fetch(`${model.baseUrl.replace(/\/+$/, "")}/wham/usage`, {
		headers: { Authorization: `Bearer ${auth.apiKey}`, ...(account ? { "chatgpt-account-id": account } : {}) },
		signal: AbortSignal.timeout(TIMEOUT_MS),
	});
	if (!response.ok) throw new Error(`Codex usage answered ${response.status}`);
	const body = await response.json() as CodexUsage;
	const windows: QuotaWindow[] = [];
	for (const window of [body.rate_limit?.primary_window, body.rate_limit?.secondary_window]) {
		if (window) windows.push({ label: span(window.limit_window_seconds), used: window.used_percent / 100, resetsAt: window.reset_at * 1000 });
	}
	const budget = body.spend_control?.individual_limit;
	if (budget?.unit === "usd") {
		const [spent, limit] = [Number(budget.used), Number(budget.limit)];
		windows.push({ label: "Budget", used: limit > 0 ? spent / limit : budget.used_percent / 100, resetsAt: budget.reset_at * 1000, spent, limit });
	}
	return windows;
}

interface CodexWindow { used_percent: number; limit_window_seconds: number; reset_at: number }

interface CodexUsage {
	rate_limit?: { primary_window?: CodexWindow | null; secondary_window?: CodexWindow | null } | null;
	spend_control?: { individual_limit?: { used: string; limit: string; used_percent: number; reset_at: number; unit: string } | null } | null;
}

function accountOf(token: string): string | undefined {
	try {
		const claims = JSON.parse(Buffer.from(token.split(".")[1] ?? "", "base64url").toString());
		return claims["https://api.openai.com/auth"]?.chatgpt_account_id;
	} catch {
		return undefined;
	}
}

function span(seconds: number): string {
	const hours = Math.round(seconds / 3600);
	if (hours < 24) return `${hours}h`;
	const days = Math.round(hours / 24);
	return days === 7 ? "Week" : `${days}d`;
}

/** Bifrost's self-service quota of the calling virtual key, or of its access profile. */
async function bifrost(_pi: ExtensionAPI, registry: Registry, model: Model<Api>): Promise<QuotaWindow[]> {
	const auth = await registry.getApiKeyAndHeaders(model);
	if (!auth.ok || !auth.apiKey) throw new Error("no Bifrost key");
	const response = await fetch(new URL("/api/governance/virtual-keys/quota", model.baseUrl), {
		headers: { Authorization: `Bearer ${auth.apiKey}` },
		signal: AbortSignal.timeout(TIMEOUT_MS),
	});
	if (!response.ok) throw new Error(`Bifrost quota answered ${response.status}`);
	const body = await response.json() as { budgets?: { max_limit: number; current_usage: number; reset_duration: string; last_reset: string }[] };
	return (body.budgets ?? []).filter(budget => budget.max_limit > 0).map(budget => {
		const resetsAt = nextReset(Date.parse(budget.last_reset), budget.reset_duration);
		return {
			label: ({ "1d": "Day", "1w": "Week", "1M": "Month" } as Record<string, string>)[budget.reset_duration] ?? budget.reset_duration,
			used: budget.current_usage / budget.max_limit,
			...(resetsAt ? { resetsAt } : {}),
			spent: budget.current_usage,
			limit: budget.max_limit,
		};
	});
}

/** Bifrost durations: Go's plus d, w, M (calendar month), Q and Y. */
function nextReset(last: number, duration: string): number | undefined {
	const match = /^(\d+)([smhdwMQY])$/.exec(duration);
	if (!match || Number.isNaN(last)) return undefined;
	const [n, unit] = [Number(match[1]), match[2]!];
	const months = { M: 1, Q: 3, Y: 12 }[unit as "M"];
	if (months) {
		const at = new Date(last);
		at.setUTCMonth(at.getUTCMonth() + n * months);
		return at.getTime();
	}
	return last + n * { s: 1, m: 60, h: 3600, d: 86400, w: 604800 }[unit as "s"]! * 1000;
}

type Bucket = { window: string; remainingFraction: number; resetTime?: string };

interface AntigravityQuota {
	groups: { displayName: string; buckets: Bucket[] }[];
}

const BUCKETS = [["5h", "5h"], ["weekly", "Week"]] as const;

function bucketWindow(bucket: Bucket, label: string): QuotaWindow {
	return { label, used: 1 - bucket.remainingFraction, ...(bucket.resetTime ? { resetsAt: Date.parse(bucket.resetTime) } : {}) };
}

/** Gemini models share "Gemini Models"; some also have a group of their own, like "Gemini 4 Argon". */
function countsAgainst(model: Model<Api>, group: string): boolean {
	const names = [model.name, model.id.replaceAll("-", " "), ...(model.id.startsWith("gemini") ? ["gemini models"] : [])];
	return names.some(name => name.toLowerCase() === group.toLowerCase());
}

/** pi-antigravity-acp-provider reads agy's `/usage`, which spends no quota. */
async function antigravity(pi: ExtensionAPI, registry: Registry, model: Model<Api>, account: boolean): Promise<QuotaWindow[]> {
	const reply = await new Promise<{ ok: true; quota: AntigravityQuota } | { ok: false; error: string }>((resolve, reject) => {
		const timer = setTimeout(() => reject(new Error("Antigravity quota timed out")), TIMEOUT_MS);
		pi.events.emit("antigravity-acp:quota:request", { reply: (result: never) => { clearTimeout(timer); resolve(result); } });
	});
	if (!reply.ok) throw new Error(reply.error);
	// agy also reports groups for models the provider doesn't offer, such as "Claude and GPT models".
	if (account) {
		const offered = registry.getAll().filter(m => m.provider === model.provider);
		return reply.quota.groups
			.filter(group => offered.some(m => countsAgainst(m, group.displayName)))
			.flatMap(group => BUCKETS.flatMap(([kind, label]) =>
				group.buckets.filter(b => b.window === kind).map(b => bucketWindow(b, `${group.displayName} · ${label}`))));
	}
	const buckets = reply.quota.groups
		.filter(group => countsAgainst(model, group.displayName))
		.flatMap(group => group.buckets);
	return BUCKETS.flatMap(([kind, label]) => {
		const tightest = buckets.filter(b => b.window === kind).sort((a, b) => a.remainingFraction - b.remainingFraction)[0];
		return tightest ? [bucketWindow(tightest, label)] : [];
	});
}
