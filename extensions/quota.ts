import type { ExtensionAPI, ExtensionContext } from "@earendil-works/pi-coding-agent";
import type { Api, Model } from "@earendil-works/pi-ai";
import { hasQuota, modelQuota } from "./lib/quota.ts";

/** The engine reads this status key; its text is `{ provider, windows }` as JSON. */
export const STATUS_KEY = "ludolph-quota";
const EVERY_MS = 60_000;

export default function (pi: ExtensionAPI) {
	let last: { provider: string; at: number } | undefined;
	let reading: string | undefined;
	const refresh = (ctx: ExtensionContext, model: Model<Api> | undefined, force: boolean) => {
		if (!model || !hasQuota(model) || reading === model.provider) return;
		if (!force && last?.provider === model.provider && Date.now() - last.at < EVERY_MS) return;
		last = { provider: model.provider, at: Date.now() };
		reading = model.provider;
		modelQuota(pi, ctx.modelRegistry, model)
			.then(windows => ctx.ui.setStatus(STATUS_KEY, JSON.stringify({ provider: model.provider, windows })))
			// Stale quota stays shown; the next turn tries again.
			.catch(() => {})
			.finally(() => { if (reading === model.provider) reading = undefined; });
	};
	pi.on("session_start", (_event, ctx) => refresh(ctx, ctx.model, true));
	pi.on("model_select", (event, ctx) => refresh(ctx, event.model, true));
	pi.on("turn_end", (_event, ctx) => refresh(ctx, ctx.model, false));
	pi.on("agent_end", (_event, ctx) => refresh(ctx, ctx.model, true));
}
