import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

/**
 * Tell the model about the pi-web-access tools it can call from codemode scripts.
 *
 * With `"toolExposure": "deferred"` and `"toolActivation": "none"` in web-search.json, pi-web-access declares none of its tools and skips its `web_enable` loader, so their
 * schemas (~1k tokens) are not sent, while scripts can still call them. But codemode doesn't list
 * deferred tools, so without this section the model wouldn't know they exist.
 */
const WEB_TOOLS = ["web_search", "source_check", "fetch_content", "get_search_content"];

export default function (pi: ExtensionAPI) {
	pi.on("before_agent_start", (event) => {
		if (!pi.getActiveTools().includes("codemode")) return;
		const registered = new Set(pi.getAllTools().map((tool) => tool.name));
		const available = WEB_TOOLS.filter((name) => registered.has(name));
		if (available.length === 0) return;
		event.systemPromptOptions.sections.web_tools = [
			`Web tools are callable only from \`codemode\` scripts: ${available.map((name) => `\`tools.${name}()\``).join(", ")}.`,
			"Use them for web research, docs lookups, and fetching URLs, GitHub repos, PDFs, and videos.",
			"`web_search` resolves to `{ responseId, fetchId, queries: [{ query, answer, error, results: [{ title, url, snippet }] }] }`.",
			"`fetch_content` resolves to `{ responseId, urls: [{ url, title, content, error }] }`, with full `content`.",
			"Get exact parameters with `await describeTool(name)`.",
		].join("\n");
	});
}
