import type { ExtensionUIContext, ModelRuntime } from "@earendil-works/pi-coding-agent";

type Interaction = Parameters<ModelRuntime["login"]>[2];
type Prompt = Parameters<Interaction["prompt"]>[0];

export interface Account {
	provider: string;
	name: string;
	oauth: boolean;
	apiKey: boolean;
	/** `stored` in auth.json (removable by sign-out), `environment`, etc. */
	source?: string;
	label?: string;
}

/**
 * After a Radius sign-in, `pi auth login` also points the Radius MCP server in mcp.json at it, which pi doesn't export,
 * so Radius signs in only from pi.
 */
const PI_ONLY = new Set(["radius"]);

export function accounts(runtime: ModelRuntime): Account[] {
	return runtime
		.getProviders()
		.flatMap(provider => {
			if (PI_ONLY.has(provider.id)) return [];
			const status = runtime.getProviderAuthStatus(provider.id);
			const oauth = Boolean(provider.auth.oauth);
			const apiKey = provider.auth.apiKey?.login !== undefined;
			if (!oauth && !apiKey && !status.configured) return [];
			const account: Account = { provider: provider.id, name: provider.name, oauth, apiKey };
			if (status.configured) {
				account.source = status.source;
				if (status.label) account.label = status.label;
			}
			return [account];
		})
		.sort((a, b) => a.name.localeCompare(b.name));
}

/**
 * Signs in as `pi auth login` does. pi's steps go to Ludolph as notifications, and its questions as
 * dialogs whose title says what pi asks for.
 */
export async function signIn(
	runtime: ModelRuntime,
	ui: ExtensionUIContext,
	id: string,
	apiKey: boolean,
	getDeviceId: () => string,
): Promise<void> {
	const provider = PI_ONLY.has(id) ? undefined : runtime.getProvider(id);
	if (!provider) throw new Error(`Ludolph can't sign in to ${id}`);
	const type = !apiKey && provider.auth.oauth ? "oauth" : provider.auth.apiKey?.login ? "api_key" : undefined;
	if (!type) throw new Error(`${provider.name} can't sign in that way`);
	const ask = (prompt: Prompt) => JSON.stringify({ ludolphSignIn: { prompt: { type: prompt.type, message: prompt.message } } });
	// pi drops a dialog whose signal aborts without telling Ludolph, so this does.
	const unanswered = <T>(prompt: Prompt, value: T | undefined): T => {
		if (value !== undefined) return value;
		if (prompt.signal?.aborted) ui.notify(JSON.stringify({ ludolphSignIn: { ended: true } }), "info");
		throw new Error("Login cancelled");
	};
	await runtime.login(
		provider.id,
		type,
		{
			async prompt(prompt) {
				if (prompt.type === "select") {
					// An account sign-in takes the browser, which pi lists first, as `pi auth login` does.
					const first = prompt.options[0];
					if (type === "oauth" && first) return first.id;
					const labels = prompt.options.map(option => option.label);
					const picked = await ui.select(ask(prompt), labels, { signal: prompt.signal });
					return unanswered(prompt, prompt.options.find(option => option.label === picked)).id;
				}
				// A pasted code races pi's browser callback, which aborts this when it wins.
				return unanswered(prompt, await ui.input(ask(prompt), prompt.placeholder, { signal: prompt.signal }));
			},
			notify(event) {
				ui.notify(JSON.stringify({ ludolphSignIn: { event } }), "info");
			},
		},
		{ getDeviceId },
	);
	// A catalog that stays empty until sign-in fills now, as after `pi auth login`.
	await runtime.refresh({ providers: [provider.id], signal: AbortSignal.timeout(15_000) }).catch(() => undefined);
}

export async function signOut(runtime: ModelRuntime, id: string): Promise<void> {
	const stored = await runtime.listCredentials();
	if (!stored.some(credential => credential.providerId === id)) throw new Error(`pi has no stored sign-in for ${id}`);
	await runtime.logout(id);
}
