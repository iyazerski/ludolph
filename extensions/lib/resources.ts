import { basename, dirname, relative } from "node:path";
import { DefaultPackageManager, SettingsManager, type ResolvedResource } from "@earendil-works/pi-coding-agent";

/**
 * Lists and switches the user's skills and extensions the way `pi config` does.
 *
 * Only user scope: Settings aren't tied to a project. Extensions loaded with `--extension`, such as
 * Ludolph's own, aren't in pi's settings, so they aren't listed.
 */

// pi's built-in extensions, as settings.md names them. resolve() lists only those it's given.
const BUILTINS = ["mcp", "llama.cpp", "codemode", "tool-search"];
const BUILTIN = "builtin:";

type Kind = "extensions" | "skills";

export interface Resource {
	kind: Kind;
	name: string;
	path: string;
	group: string;
	enabled: boolean;
}

export interface Change {
	kind: Kind;
	path: string;
	enabled: boolean;
}

export async function resources(agentDir: string, cwd: string, change?: Change): Promise<Resource[]> {
	const settings = SettingsManager.create(cwd, agentDir);
	const packages = new DefaultPackageManager({ cwd, agentDir, settingsManager: settings, builtinExtensions: BUILTINS });
	let resolved = await packages.resolve(async () => "skip");
	if (change) {
		// A settings file pi couldn't read would be overwritten with only what it did read.
		throwSettingsErrors(settings);
		const found = resolved[change.kind].find(r => r.path === change.path && r.metadata.scope === "user");
		if (!found) throw new Error(`${change.path} isn't one of your ${change.kind}`);
		switchResource(settings, agentDir, change.kind, found, change.enabled);
		await settings.flush();
		// flush() records a failed write instead of throwing it.
		throwSettingsErrors(settings);
		resolved = await packages.resolve(async () => "skip");
	}
	const kinds: Kind[] = ["extensions", "skills"];
	return kinds.flatMap(kind =>
		resolved[kind]
			.filter(r => r.metadata.scope === "user")
			.map(r => ({ kind, name: name(kind, r), path: r.path, group: group(r, agentDir), enabled: r.enabled })),
	);
}

function throwSettingsErrors(settings: SettingsManager) {
	const [first] = settings.drainErrors();
	if (first) throw new Error(`pi's settings at ${first.path ?? first.scope}: ${first.error.message}`);
}

// Mirrors pi config's toggleTopLevelResource and togglePackageResource for user scope.
function switchResource(settings: SettingsManager, agentDir: string, kind: Kind, item: ResolvedResource, enabled: boolean) {
	const global = settings.getGlobalSettings();
	const { metadata } = item;
	if (metadata.origin === "top-level") {
		const pattern = metadata.source === "builtin" ? item.path : relative(metadata.baseDir ?? agentDir, item.path);
		const paths = [...withoutPattern(global[kind] ?? [], pattern), `${enabled ? "+" : "-"}${pattern}`];
		if (kind === "extensions") settings.setExtensionPaths(paths);
		else settings.setSkillPaths(paths);
		return;
	}
	const sources = [...(global.packages ?? [])];
	const at = sources.findIndex(p => (typeof p === "string" ? p : p.source) === metadata.source);
	if (at === -1) throw new Error(`${metadata.source} isn't in your pi settings`);
	const source = sources[at];
	const pkg = typeof source === "string" ? { source } : { ...source };
	const pattern = relative(metadata.baseDir ?? dirname(item.path), item.path);
	const filters = [...withoutPattern(pkg[kind] ?? [], pattern), `${enabled ? "+" : "-"}${pattern}`];
	pkg[kind] = filters;
	sources[at] = pkg;
	settings.setPackages(sources);
}

function withoutPattern(patterns: string[], pattern: string): string[] {
	return patterns.filter(p => (/^[!+-]/.test(p) ? p.slice(1) : p) !== pattern);
}

// The names pi config shows, except that a package's index file takes the package's name.
function name(kind: Kind, { path, metadata }: ResolvedResource): string {
	if (path.startsWith(BUILTIN)) return path.slice(BUILTIN.length);
	const file = basename(path);
	if (metadata.origin === "package" && metadata.baseDir && /^index\.[jt]s$/.test(file)) return basename(metadata.baseDir);
	const folder = basename(dirname(path));
	if (kind === "extensions" && folder !== "extensions") return `${folder}/${file}`;
	if (kind === "skills" && file === "SKILL.md") return folder;
	return file;
}

function group({ metadata }: ResolvedResource, agentDir: string): string {
	if (metadata.origin === "package") return metadata.source;
	if (metadata.source === "builtin") return "Built-in";
	if (metadata.source === "auto") return home(metadata.baseDir ?? agentDir);
	return "Your pi settings";
}

function home(path: string): string {
	const root = process.env.HOME;
	return root && path.startsWith(root) ? `~${path.slice(root.length)}` : path;
}
