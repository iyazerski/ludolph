import { chmod, copyFile, lstat, mkdir, mkdtemp, readdir, rm } from "node:fs/promises";
import { basename, join, resolve } from "node:path";
import { homedir, tmpdir } from "node:os";

/** Bridge local files to GUI servers without using a repository. browse reaches only the temp folders. */
export class GuiArtifacts {
	private constructor(
		readonly directory: string,
		readonly inputs: { source: string; staged: string }[],
	) {}

	static async create(files: string[], cwd: string, root = join(tmpdir(), "pi-gui")): Promise<GuiArtifacts> {
		await mkdir(root, { recursive: true, mode: 0o700 });
		const directory = await mkdtemp(join(root, "task-"));
		const artifacts = new GuiArtifacts(directory, []);
		try {
			await mkdir(join(directory, "inputs"), { mode: 0o700 });
			await mkdir(join(directory, "outputs"), { mode: 0o700 });
			for (const [index, file] of files.entries()) {
				const source = resolve(cwd, file.replace(/^@/, "").replace(/^~(?=\/|$)/, homedir()));
				if (!(await lstat(source)).isFile()) throw new Error(`GUI input must be a regular file: ${source}`);
				const staged = join(directory, "inputs", `${index}-${basename(source)}`);
				await copyFile(source, staged);
				await chmod(staged, 0o600);
				artifacts.inputs.push({ source, staged });
			}
			return artifacts;
		} catch (error) {
			await artifacts.dispose();
			throw error;
		}
	}

	instructions(): string {
		return [
			`Save screenshots and other outputs directly in ${join(this.directory, "outputs")}, with flat filenames.`,
			"Use this directory even if the task names another path. The caller exports outputs to temporary paths and reports them.",
			"Never save artifacts in a repository. If file access is denied, report the error. Don't switch directories.",
			...this.inputs.map(({ source, staged }) => `Input ${JSON.stringify(source)} is staged at ${JSON.stringify(staged)}. Use the staged path.`),
		].join("\n");
	}

	async exportReport(report: string): Promise<string> {
		const output = join(this.directory, "outputs");
		const entries = await readdir(output, { withFileTypes: true });
		if (!entries.length) return report;
		if (entries.some((entry) => !entry.isFile())) throw new Error("GUI outputs must be regular files, not directories or symlinks");
		const destination = await mkdtemp(join("/tmp", "pi-gui-"));
		const paths: string[] = [];
		try {
			for (const entry of entries) {
				const source = join(output, entry.name);
				const target = join(destination, entry.name);
				await copyFile(source, target);
				await chmod(target, 0o600);
				report = report.replaceAll(source, target);
				paths.push(target);
			}
			return `${report}\n\nArtifacts:\n${paths.join("\n")}`;
		} catch (error) {
			await rm(destination, { recursive: true, force: true });
			throw error;
		}
	}

	async dispose(): Promise<void> {
		await rm(this.directory, { recursive: true, force: true });
	}
}
