import { expect, test } from "bun:test";
import { join, dirname } from "node:path";
import { mkdtemp, readFile, readdir, rm, stat, symlink, writeFile } from "node:fs/promises";
import gui from "../gui.ts";
import { GuiArtifacts } from "../lib/gui-artifacts.ts";
import { withTestSession } from "./helpers.ts";

test("stages explicit GUI inputs privately, exports outputs, and cleans staging", async () => {
	const root = await mkdtemp(join("/tmp", "pi-gui-test-"));
	let exported: string | undefined;
	try {
		await writeFile(join(root, "upload.txt"), "upload");
		const artifacts = await GuiArtifacts.create(["upload.txt"], root, join(root, "bridge"));
		try {
			expect(await readFile(artifacts.inputs[0].staged, "utf8")).toBe("upload");
			expect((await stat(artifacts.directory)).mode & 0o777).toBe(0o700);
			expect((await stat(artifacts.inputs[0].staged)).mode & 0o777).toBe(0o600);
			const screenshot = join(artifacts.directory, "outputs", "screen.png");
			await writeFile(screenshot, "image");
			const report = await artifacts.exportReport("Status: done\nEvidence: " + screenshot);
			exported = report.split("Artifacts:\n")[1].trim();
			expect(exported.startsWith(join("/tmp", "pi-gui-"))).toBe(true);
			expect(await readFile(exported, "utf8")).toBe("image");
			expect(report).not.toContain(artifacts.directory);
		} finally {
			await artifacts.dispose();
		}
		expect(await readdir(join(root, "bridge"))).toEqual([]);
		await expect(GuiArtifacts.create(["missing"], root, join(root, "bridge"))).rejects.toThrow();
		expect(await readdir(join(root, "bridge"))).toEqual([]);
		const unsafe = await GuiArtifacts.create([], root, join(root, "bridge"));
		try {
			await symlink(join(root, "upload.txt"), join(unsafe.directory, "outputs", "linked"));
			await expect(unsafe.exportReport("Status: done")).rejects.toThrow("regular files");
		} finally {
			await unsafe.dispose();
		}
	} finally {
		if (exported) await rm(dirname(exported), { recursive: true, force: true });
		await rm(root, { recursive: true, force: true });
	}
});

test("rejects unavailable GUI models", async () => {
	await withTestSession([gui], async (session) => {
		const runner = session.extensionRunner;
		const browser = runner.getToolDefinition("browser_use");
		if (!browser) throw new Error("Browser tool not registered");
		await expect(browser.execute("call", { task: "Read the page", model: "missing/gui" }, undefined, undefined, runner.createToolContext("call", undefined))).rejects.toThrow("unavailable");
	});
});
