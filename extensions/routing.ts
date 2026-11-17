import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import toolHints from "./lib/tool-hints.ts";
import toolRouting from "./lib/tool-routing.ts";
import webTools from "./lib/web-tools.ts";

export default function (pi: ExtensionAPI) {
	toolRouting(pi);
	toolHints(pi);
	webTools(pi);
}
