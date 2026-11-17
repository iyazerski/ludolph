import * as path from "node:path";
import { fileURLToPath } from "node:url";
import { StringEnum } from "@earendil-works/pi-ai";
import { type ExtensionAPI, type ExtensionContext, getAgentDir } from "@earendil-works/pi-coding-agent";
import { Type } from "typebox";
import { resolveModel, runPi, THINKING_LEVELS } from "./lib/child-pi.ts";
import { GuiArtifacts } from "./lib/gui-artifacts.ts";

/**
 * Run desktop and browser automation in subagents instead of the main agent.
 *
 * Screenshots, accessibility trees, and DOM snapshots fill the context fast, and the main agent
 * only needs the outcome. So the cua-driver and browse MCP servers are not connected in the main
 * session at all. Instead, `computer_use` and `browser_use` hand a task to a child pi
 * process that sees only its own server's tools and returns a short report. Each tool keeps one
 * child session per main session, so follow-up calls continue on the same app or page.
 *
 * The child loads only this extension, MCP support, and codemode when its role uses it, with
 * PI_GUI_SUBAGENT naming its role. There it registers its MCP server and sets the role prompt.
 * Codemode suits cua-driver: requests leave out its 62 tool schemas, and scripts batch steps and
 * filter large snapshots. browse already runs scripts in its `execute` tool, and was faster and
 * cheaper with direct tools. The servers must not also be in mcp.json, or the main agent would
 * reach them too.
 *
 * Ludolph sets LUDOLPH_GUI_TOOLS to the roles turned on in its settings, and LUDOLPH_GUI_MODEL and
 * LUDOLPH_GUI_THINKING to the model picked for them. Without a model, the child uses pi's default.
 */

const ROLE_ENV = "PI_GUI_SUBAGENT";
const MODEL = process.env.LUDOLPH_GUI_MODEL || undefined;
const THINKING = THINKING_LEVELS.find((level) => level === process.env.LUDOLPH_GUI_THINKING) ?? "medium";
const TIMEOUT_MS = 20 * 60_000;
const SESSION_DIR = path.join(getAgentDir(), "gui-subagent-sessions");
// How long the subagent waits for its MCP server before the first request.
const SERVER_WAIT_MS = 15_000;

const REPORT_FORMAT = `The caller sees only your final message. Return a short report:

Status: done | failed | needs_input
Result: findings or actions, with exact values.
Evidence: supporting URLs, visible text, or file paths.
Notes: side effects, blockers, or other essential caveats.`;

const SAFETY = `Never send messages, post, submit forms, buy, delete, or change settings or accounts unless the task explicitly asks for it. Stop and report \`needs_input\` when a login, 2FA, CAPTCHA, permission prompt, or an unclear choice blocks you. Don't guess.`;

interface Role {
	tool: string;
	label: string;
	server: string;
	serverConfig: { command: string; args: string[] };
	description: string;
	prompt: string;
	// `deferred` keeps a rarely used tool out of every request, schema and codemode listing alike,
	// like the web tools: the gui_subagents prompt section names it instead.
	exposure: "direct" | "deferred";
	// A rule for the main agent, added to the gui_subagents prompt section while the tool is reachable.
	mainRule?: string;
	// How the child reaches the server's tools.
	serverExposure: "codemode" | "direct";
}

const ROLES: Record<string, Role> = {
	computer: {
		tool: "computer_use",
		label: "Computer Use",
		server: "cua-driver",
		serverConfig: { command: "cua-driver", args: ["mcp"] },
		description: [
			"Delegate macOS app, window, menu, and dialog tasks to a cua-driver subagent on the user's desktop.",
			"Returns Status (done, failed, needs_input), Result, Evidence, and Notes. State persists between calls.",
			"Prefer CLI/API/file access when possible; use browser_use for Chrome pages.",
		].join(" "),
		exposure: "deferred",
		serverExposure: "codemode",
		// Main agents fell back to osascript, which hung on a macOS automation permission prompt.
		mainRule:
			"For macOS UI inspection and control, use `computer_use`, not osascript, AppleScript, JXA, or other UI scripting: permission prompts can block until the user responds.",
		prompt: `Use cua-driver on the user's macOS desktop: query windows narrowly, act by element token with run_steps, and verify at checkpoints. Call get_guide when unsure. Don't quit apps or close windows you didn't open.

${SAFETY}

${REPORT_FORMAT}`,
	},
	browser: {
		tool: "browser_use",
		label: "Browser Use",
		server: "browse",
		serverConfig: { command: "browse", args: ["mcp"] },
		description: [
			"Delegate interactive Chrome tasks, including forms, console/network inspection, and debugging, to a browse subagent in the user's running Chrome.",
			"Returns Status (done, failed, needs_input), Result, Evidence, and Notes. State persists between calls.",
			"Request screenshots for visual proof. For page content without interaction or login, prefer web tools.",
		].join(" "),
		exposure: "direct",
		serverExposure: "direct",
		prompt: `Use browse in the user's running Chrome:
- Work only in tabs you open, and pass the tab id from each execute result to the next call. Close your tabs when done unless asked to keep them.
- Batch dependent steps in one execute script and return only what you need.
- Read pages with snapshot(). Take screenshots only for layout checks or requested visual proof.
- Don't call handoff(): report needs_input instead.
- If no browser is connected, report failed with the nextStep from status.

${SAFETY}

${REPORT_FORMAT}`,
	},
};

const params = Type.Object({
	task: Type.String({
		description: "Self-contained task: the goal, the app or URL, input values, and what to report back.",
	}),
	files: Type.Optional(Type.Array(Type.String(), { description: "Local files to upload or open; staged privately for the child." })),
	model: Type.Optional(Type.String({ description: "Per-call provider/model override. No automatic fallback." })),
	thinking: Type.Optional(StringEnum(THINKING_LEVELS, { description: "Per-call reasoning level." })),
});

/** Return the prefix pi gives the tools of an MCP server. */
function toolPrefix(server: string): string {
	return `mcp__${server.replace(/-/g, "_")}__`;
}

/** Resolve the requested model to an available, image-capable `provider/model`. */
function guiModel(ctx: ExtensionContext, requested: string): string {
	const model = resolveModel(ctx, requested);
	if (!model) throw new Error(`GUI model ${requested} is unavailable. Pick an available model in Ludolph's settings or pass the model argument.`);
	if (!model.images) throw new Error(`GUI model ${requested} does not support images`);
	return model.id;
}

/** Register the role's MCP server and prompt: this process is a GUI subagent. */
function setUpSubagent(pi: ExtensionAPI, role: Role): void {
	pi.registerMcpServer(role.server, { ...role.serverConfig, exposure: role.serverExposure });
	const prefix = toolPrefix(role.server);

	pi.on("before_agent_start", async (event) => {
		const { sections } = event.systemPromptOptions;
		sections.gui_subagent = role.serverExposure === "direct" ? role.prompt : `${role.prompt}

Call ${role.server} tools only in \`codemode\`: \`tools.${prefix}<name>(args)\`. Inspect parameters with \`describeTool(name)\`. Batch steps needing no intermediate inspection. Filter large results before returning them. View screenshots with \`image(result.content[i])\`.`;
		// Pi lists server instructions only for codemode and deferred servers, and the server may
		// still be connecting. It connects in well under a second, so wait for its tools.
		const deadline = Date.now() + SERVER_WAIT_MS;
		let tool = pi.getAllTools().find((candidate) => candidate.name.startsWith(prefix));
		while (!tool && Date.now() < deadline) {
			await new Promise((resolve) => setTimeout(resolve, 100));
			tool = pi.getAllTools().find((candidate) => candidate.name.startsWith(prefix));
		}
		const instructions = tool?.namespace?.instructions;
		if (instructions) {
			sections.server_instructions = `<server_instructions server="${role.server}">\n${instructions}\n</server_instructions>`;
		}
	});
}

/** Register the delegating tools in the main session. */
function setUpMain(pi: ExtensionAPI, roles: Record<string, Role>, extensionPath: string): void {
	for (const [roleName, role] of Object.entries(roles)) {
		pi.registerTool({
			name: role.tool,
			label: role.label,
			description: role.description,
			parameters: params,
			exposure: role.exposure,
			// Calls of one tool share a child session, so they can't run in parallel.
			executionMode: "sequential",

			async execute(_toolCallId, { task, files, model: requestedModel, thinking }, signal, onUpdate, ctx) {
				// One child session per main session and role, kept across calls.
				const sessionId = `${ctx.sessionManager.getSessionId()}-${roleName}`;
				const requested = requestedModel ?? MODEL;
				const model = requested ? guiModel(ctx, requested) : undefined;
				const level = THINKING_LEVELS.find((candidate) => candidate === thinking) ?? THINKING;
				const args = [
					"--session-dir",
					SESSION_DIR,
					"--session-id",
					sessionId,
					...(model ? ["--model", model] : []),
					"--thinking",
					level,
					// Only this extension, MCP, and codemode: no other tools, skills, or prompt templates.
					"--no-extensions",
					"-e",
					"builtin:mcp",
					...(role.serverExposure === "codemode" ? ["-e", "builtin:codemode"] : []),
					"-e",
					extensionPath,
					"--no-skills",
					"--no-prompt-templates",
					// AGENTS.md is about coding: the role prompt carries the rules that apply here.
					"--no-context-files",
					// Declares only codemode or the server's tools, and scripts reach only this role's MCP server.
					"--tools",
					role.serverExposure === "codemode" ? `codemode,${toolPrefix(role.server)}*` : `${toolPrefix(role.server)}*`,
				];
				const prefix = toolPrefix(role.server);
				let steps = 0;
				onUpdate?.({ content: [{ type: "text", text: "starting…" }], details: undefined });

				const artifacts = await GuiArtifacts.create(files ?? [], ctx.cwd);
				try {
					const result = await runPi({
						name: role.tool,
						args,
						input: `${artifacts.instructions()}\n\nTask:\n${task}`,
						cwd: ctx.cwd,
						env: { ...process.env, [ROLE_ENV]: roleName },
						signal,
						timeoutMs: TIMEOUT_MS,
						onToolStart: (tool) => {
							steps += 1;
							const name = tool.startsWith(prefix) ? tool.slice(prefix.length) : tool;
							onUpdate?.({ content: [{ type: "text", text: `${steps} steps, last: ${name}` }], details: undefined });
						},
					});
					let { text, failed } = result;
					try {
						text = await artifacts.exportReport(text);
					} catch (error) {
						failed = true;
						text = `Status: failed\nResult: GUI artifact export failed.\nNotes: ${String(error)}\n\nSubagent report:\n${text}`;
					}
					return {
						content: [{ type: "text", text }],
						details: undefined,
						// Count the child's tokens in this session's totals.
						usage: result.usage,
						isError: failed,
					};
				} finally {
					await artifacts.dispose();
				}
			},
		});
	}

	pi.on("before_agent_start", (event) => {
		const active = pi.getActiveTools();
		// Deferred tools are reached here from codemode scripts, so only while codemode is active.
		const reachable = (role: Role): boolean =>
			active.includes(role.exposure === "deferred" ? "codemode" : role.tool);
		const tools = Object.values(roles).filter(reachable);
		if (tools.length === 0) return;
		const name = (role: Role): string =>
			role.exposure === "deferred" ? `\`tools.${role.tool}({ task })\` in \`codemode\` (${role.server})` : `\`${role.tool}\` (${role.server})`;
		// Instructions shared across harnesses may name these MCP servers: point the model to the tools.
		event.systemPromptOptions.sections.gui_subagents = [
			`GUI automation uses subagents: ${tools.map(name).join(", ")}.`,
			"Use these tools even when instructions name their MCP servers; those servers aren't connected here.",
			"Pass input paths in `files`; outputs are exported to temporary paths in the result. Never stage GUI artifacts in a repository.",
			"Change models/reasoning with `model`/`thinking`, not shell wrappers. Change providers only when asked.",
			"Check Status and Evidence before relying on results.",
			...tools.flatMap((role) => (role.mainRule ? [role.mainRule] : [])),
		].join("\n");
	});
}

export default function (pi: ExtensionAPI) {
	const roleName = process.env[ROLE_ENV];
	if (roleName) {
		const role = ROLES[roleName];
		if (!role) throw new Error(`Unknown ${ROLE_ENV}: ${roleName}`);
		setUpSubagent(pi, role);
		return;
	}
	const enabled = process.env.LUDOLPH_GUI_TOOLS?.split(",");
	const roles = Object.fromEntries(Object.entries(ROLES).filter(([name]) => !enabled || enabled.includes(name)));
	// Child processes load this same module and pick their role by name.
	setUpMain(pi, roles, fileURLToPath(import.meta.url));
}
