import { mkdtemp, rm } from "node:fs/promises";
import { join } from "node:path";
import { createAgentSession, DefaultResourceLoader, ModelRuntime, SessionManager, SettingsManager, type AgentSession, type ExtensionFactory } from "@earendil-works/pi-coding-agent";
import { InMemoryCredentialStore, type AssistantMessage, type ToolCall } from "@earendil-works/pi-ai";

const usage = { input: 1000, output: 0, cacheRead: 0, cacheWrite: 0, totalTokens: 1000, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0, total: 0 } };

export function assistant(calls: ToolCall[]): AssistantMessage {
	return { role: "assistant", content: calls, api: "openai-completions", provider: "test", model: "gui", usage, stopReason: "toolUse", timestamp: 0 };
}

export type ProviderInput = Parameters<ModelRuntime["registerProvider"]>[1];

const testProvider: ProviderInput = { api: "openai-completions", baseUrl: "http://127.0.0.1:1", apiKey: "test",
	models: [{ id: "gui", name: "GUI", input: ["text", "image"], reasoning: true, cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 }, contextWindow: 10000, maxTokens: 1000 }] };

export async function withTestSession(factories: ExtensionFactory[], run: (session: AgentSession, root: string) => Promise<void>,
	providers: Record<string, ProviderInput> = { test: testProvider }): Promise<void> {
	const root = await mkdtemp(join("/tmp", "pi-setup-test-"));
	let session: AgentSession | undefined;
	try {
		const settingsManager = SettingsManager.inMemory();
		const loader = new DefaultResourceLoader({ cwd: root, agentDir: root, settingsManager,
			noExtensions: true, noSkills: true, noPromptTemplates: true, noThemes: true, noContextFiles: true,
			extensionFactories: factories });
		await loader.reload();
		if (loader.getExtensions().errors.length) throw new Error(JSON.stringify(loader.getExtensions().errors));
		const runtime = await ModelRuntime.create({ credentials: new InMemoryCredentialStore(), modelsPath: null, refreshOnCreate: false });
		for (const [id, config] of Object.entries(providers)) runtime.registerProvider(id, config);
		({ session } = await createAgentSession({ cwd: root, agentDir: root, modelRuntime: runtime,
			resourceLoader: loader, settingsManager, sessionManager: SessionManager.inMemory(root) }));
		await run(session, root);
	} finally {
		session?.dispose();
		await rm(root, { recursive: true, force: true });
	}
}
