import { expect, test } from "bun:test";
import { join } from "node:path";
import { mkdir, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { resources } from "../resources.ts";

test("lists user skills and built-in extensions, and switches them as pi config does", async () => {
	const root = await mkdtemp(join("/tmp", "pi-resources-test-"));
	try {
		const agentDir = join(root, "agent");
		const skill = join(agentDir, "skills", "review", "SKILL.md");
		await mkdir(join(agentDir, "skills", "review"), { recursive: true });
		await writeFile(skill, "---\nname: review\ndescription: Review code.\n---\nReview.\n");
		await writeFile(join(agentDir, "settings.json"), "{}");

		const listed = await resources(agentDir, root);
		expect(listed.find(r => r.path === skill)).toMatchObject({ kind: "skills", name: "review", enabled: true });
		expect(listed.filter(r => r.group === "Built-in").map(r => r.name)).toEqual(["mcp", "llama.cpp", "codemode", "tool-search"]);

		const off = await resources(agentDir, root, { kind: "skills", path: skill, enabled: false });
		expect(off.find(r => r.path === skill)?.enabled).toBe(false);
		const mcpOff = await resources(agentDir, root, { kind: "extensions", path: "builtin:mcp", enabled: false });
		expect(mcpOff.find(r => r.path === "builtin:mcp")?.enabled).toBe(false);
		const saved = JSON.parse(await readFile(join(agentDir, "settings.json"), "utf8"));
		expect(saved.skills).toEqual(["-skills/review/SKILL.md"]);
		expect(saved.extensions).toEqual(["-builtin:mcp"]);

		const on = await resources(agentDir, root, { kind: "skills", path: skill, enabled: true });
		expect(on.find(r => r.path === skill)?.enabled).toBe(true);
		await expect(resources(agentDir, root, { kind: "skills", path: "/nope/SKILL.md", enabled: false })).rejects.toThrow();
	} finally {
		await rm(root, { recursive: true, force: true });
	}
});
