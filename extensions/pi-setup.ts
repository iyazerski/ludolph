import { getAgentDir, type ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { accounts } from "./lib/accounts.ts";
import { resources, type Change } from "./lib/resources.ts";

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
				const { topic, change } = JSON.parse(args) as { topic: "resources" | "accounts"; change?: Change };
				const ok = topic === "accounts" ? accounts(ctx.modelRegistry) : await resources(getAgentDir(), ctx.cwd, change);
				answer = { ok };
			} catch (e) {
				answer = { error: e instanceof Error ? e.message : String(e) };
			}
			ctx.ui.notify(JSON.stringify({ ludolphSetup: answer }), "info");
		},
	});
}
