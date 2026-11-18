import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { StringEnum } from "@earendil-works/pi-ai";
import { createConnection } from "node:net";
import { Type } from "typebox";

const RESULT_COMMAND = "ludolph-job-result";
const actions = Type.Object({
	command: Type.Optional(Type.String({ description: "Bash hook in the same project. It shares the job timeout. Check assumptions before changing files or branches." })),
	notify_user: Type.Optional(Type.Boolean({ description: "Send a Ludolph desktop and linked-phone notification." })),
	resume_chat: Type.Optional(Type.String({ description: "What the agent should do with the result. Omit unless a new turn is needed." })),
});
const parameters = Type.Object({
	action: StringEnum(["start", "list", "status", "cancel"] as const),
	id: Type.Optional(Type.String({ description: "Job ID, required for status and cancel." })),
	title: Type.Optional(Type.String({ description: "Required for start. Short, user-facing title. Do not include secrets." })),
	command: Type.Optional(Type.String({ description: "Required for start. Bash command in the chat's project. Do not detach it." })),
	timeout_seconds: Type.Optional(Type.Integer({ minimum: 1, maximum: 86400, description: "Required for start. Time limit for the command and its hook together." })),
	on_success: Type.Optional({ ...actions, description: "Run this hook after a successful command. Notify or resume only if both succeed." }),
	on_failure: Type.Optional({ ...actions, description: "Run this hook only if the main command exits unsuccessfully before the timeout. Notify or resume for any failure or timeout, including a failed success hook." }),
});

function call(path: string, chat: string, request: unknown): Promise<unknown> {
	return new Promise((resolve, reject) => {
		const socket = createConnection(path);
		let reply = "";
		socket.setEncoding("utf8").setTimeout(5000);
		socket.on("connect", () => socket.write(JSON.stringify({ id: 1, call: "background_job", args: { chat, request } }) + "\n"));
		socket.on("error", reject);
		socket.on("timeout", () => socket.destroy(new Error("Ludolph did not answer in time. List jobs before retrying a start.")));
		socket.on("close", () => reject(new Error("Ludolph disconnected. List jobs before retrying a start.")));
		socket.on("data", (chunk: string) => {
			reply += chunk;
			const end = reply.indexOf("\n");
			if (end < 0) return;
			socket.destroy();
			try {
				const result = JSON.parse(reply.slice(0, end));
				if (typeof result.error === "string") reject(new Error(result.error));
				else resolve(result.ok);
			} catch (error) {
				reject(error);
			}
		});
	});
}

export default function (pi: ExtensionAPI) {
	const socket = process.env.LUDOLPH_ENGINE_SOCKET;
	if (!socket) return;
	pi.registerTool({
		name: "background_job",
		label: "Background job",
		description: "Start, list, inspect, or cancel engine-owned Bash jobs in this chat. Starting returns a job ID immediately. Use notify_user or resume_chat in on_success or on_failure for outcome-specific follow-ups. Cancelled jobs run no further actions.",
		promptSnippet: "Run long work without blocking this turn, with optional user notices and chat continuation.",
		promptGuidelines: [
			"Use background_job for long commands, remote checks, and follow-ups instead of detaching shell scripts or repeatedly polling. Use the same approval rules as for Bash, including for hooks and continuations.",
			"When asked to clean up a branch after a PR merges, use on_success.command. Before switching to the default branch and deleting the local branch, verify the PR merged at the expected head and the worktree is clean and still on that branch at that head. Stop if a check fails.",
		],
		parameters,
		async execute(_id, request, _signal, _update, ctx) {
			const result = await call(socket, ctx.sessionManager.getSessionId(), request);
			return { content: [{ type: "text", text: JSON.stringify(result) }], details: undefined };
		},
	});
	pi.registerCommand(RESULT_COMMAND, {
		description: "Deliver an engine background job result",
		async handler(args) {
			const content: unknown = JSON.parse(args);
			if (typeof content !== "string" || !content.startsWith("Ludolph background job: ")) throw new Error("Not a Ludolph background job result");
			pi.sendMessage({ customType: RESULT_COMMAND, content, display: true }, { triggerTurn: true, deliverAs: "followUp" });
		},
	});
}
