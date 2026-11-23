import { getAgentDir, ModelRuntime, SettingsManager, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { accounts, signIn, signOut } from "./lib/accounts.ts";
import { accountQuota } from "./lib/quota.ts";
import { resources, type Change } from "./lib/resources.ts";

type Request =
	| { topic: "resources"; change?: Change }
	| { topic: "accounts" }
	| { topic: "quota" }
	| { topic: "signIn"; provider: string; apiKey: boolean }
	| { topic: "signOut"; provider: string };

/**
 * Answers Ludolph's Settings about pi's own setup. The engine runs it in a pi with no session, sends
 * one command, and reads the answer from a notification.
 */
export default function (pi: ExtensionAPI) {
	pi.registerCommand("ludolph-setup", {
		description: "Answer Ludolph's Settings about pi's setup",
		handler: async (args, ctx) => {
			let answer;
			try {
				const request = JSON.parse(args) as Request;
				if (request.topic === "resources") {
					answer = { ok: await resources(getAgentDir(), ctx.cwd, request.change) };
				} else if (request.topic === "quota") {
					answer = { ok: await accountQuota(pi, ctx.modelRegistry) };
				} else {
					// The runtime `pi auth` uses, which has providers whose catalog fills only after sign-in.
					const runtime = await ModelRuntime.create({ allowModelNetwork: false, signal: AbortSignal.timeout(15_000) });
					if (request.topic === "accounts") {
						answer = { ok: accounts(runtime) };
					} else if (request.topic === "signOut") {
						await signOut(runtime, request.provider);
						answer = { ok: null };
					} else {
						const settings = SettingsManager.create(ctx.cwd, getAgentDir());
						await signIn(runtime, ctx.ui, request.provider, request.apiKey, () => settings.getOrCreateDeviceId());
						answer = { ok: null };
					}
				}
			} catch (e) {
				answer = { error: e instanceof Error ? e.message : String(e) };
			}
			ctx.ui.notify(JSON.stringify({ ludolphSetup: answer }), "info");
		},
	});
}
