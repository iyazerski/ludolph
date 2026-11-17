import type { ModelRegistry } from "@earendil-works/pi-coding-agent";

export interface Account {
	provider: string;
	name: string;
	/** Signs in with an account in the browser. */
	oauth: boolean;
	/** Takes an API key. */
	apiKey: boolean;
	/** Where pi finds its credentials: `stored` in auth.json, which sign-out removes, `environment`, and so on. */
	source?: string;
	/** The environment variable or setting that holds them, when pi names one. */
	label?: string;
}

/**
 * Providers that can sign in or already have credentials, as `pi auth login` and `/login` offer them.
 *
 * pi lists providers through their models, so one whose catalog stays empty until sign-in is missing.
 */
export function accounts(registry: ModelRegistry): Account[] {
	const ids = [...new Set(registry.getAll().map(model => model.provider))];
	return ids
		.flatMap(provider => {
			const auth = registry.getProvider(provider)?.auth;
			const status = registry.getProviderAuthStatus(provider);
			const oauth = Boolean(auth?.oauth);
			const apiKey = auth?.apiKey?.login !== undefined;
			if (!oauth && !apiKey && !status.configured) return [];
			const account: Account = { provider, name: registry.getProviderDisplayName(provider), oauth, apiKey };
			if (status.configured) {
				account.source = status.source;
				if (status.label) account.label = status.label;
			}
			return [account];
		})
		.sort((a, b) => a.name.localeCompare(b.name));
}
