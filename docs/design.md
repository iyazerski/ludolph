# Ludolph design

Status: draft.

## Goal

Ludolph is a Mac app for [pi](https://pi.dev) coding agents. It replaces two things: Zeron (its engine and app) and zeron-tools (`connect` and the Companion). It should stay simple, and it should be light on CPU and memory.

It keeps what zeron-tools has today, and nothing more from Zeron:

- chats grouped by project, with live transcripts, tool-call rows, each turn's changes, and the composer (images, dictation, skills, model and thinking level, stop, archive)
- questions from the agent, answered in the app
- titles and archiving as part of the main flow, not as workarounds
- a floating quick window on a global shortcut (today's Companion)
- a phone web app through a self-hosted relay, with notifications

### Not in scope

- Agents other than pi.
- Anything Zeron has that the Companion doesn't: other harnesses, accounts and sync, PR and CI views, terminals, worktrees, pinned chats, and sidebar sections.
- Windows and Linux.

## What we start from

Measured on the current setup (pi 1.5.2, Zeron's engine as a service, zeron-tools 0.19):

| Process | Memory (RSS) |
|---|---|
| Zeron Engine (`headless`) | ~600 MB |
| zeron-tools companion | ~128 MB |
| zeron-tools connect | ~48 MB |
| one `pi --mode rpc` with a session loaded | ~220 MB |
| one fresh `pi --mode rpc --no-session` | ~135 MB |
| `pi --mode rpc --session` on the largest session here (45 MB, 237 messages) | ~300 MB |

A fresh `pi --mode rpc` answers its first `get_state` in about 250 ms. Resuming that 45 MB session takes about 270 ms, so session size barely changes the start time.

So memory goes on two things: Zeron's engine, which Ludolph removes, and one pi process for every chat, which Ludolph can stop when the chat is idle (see [pi processes](#pi-processes)).

## Processes

```
                 ┌──────────────── Ludolph.app ────────────────┐
 phone ──HTTPS──▶ relay ◀──WebSocket── engine ◀──socket── app (main + quick window)
                                         │
                                         ├── pi --mode rpc  (one per active chat)
                                         ├── pi -p          (titles, dictation cleanup)
                                         └── git            (turn changes)
```

- **Engine** (`ludolph engine`): a launch agent. It starts at login, restarts after a crash, and runs whether a window is open or not. It owns the pi processes, the index, titles, archiving, turn changes, and the relay link. It has no UI.
- **App** (`Ludolph.app`): one UI process with both windows. It starts at login, so the global shortcut always works. It shows a Dock icon only while the main window is open, and runs as an accessory app otherwise. Rune supports several windows in one app and switches between the regular and accessory activation policies (checked in Rune's source at the pinned revision).
- **Relay**: a small server you host. It's today's `zeron-relay`, moved into this repo.

Why the engine is its own process: agents and the phone keep working when the app is quit, crashes, or updates. That's the reason Zeron's engine was moved to a service in zeron-tools 0.19.

Both processes run from the same binary inside `Ludolph.app`.

## Sources of truth

| Data | Where | Owner |
|---|---|---|
| Transcript, cwd, model, thinking level, title | pi's session files, `~/.pi/agent/sessions/**.jsonl` | pi |
| Index: one row per session | `~/Library/Application Support/Ludolph/index.json` | engine |
| Turn changes | `~/Library/Application Support/Ludolph/turns/<session id>/<entry id>.json` | engine |
| Settings | `~/Library/Application Support/Ludolph/settings.json` | app writes, engine reads |
| UI state (seen turns, folded projects, window frames, last picks) | `~/Library/Application Support/Ludolph/ui.json` | app |

Ludolph never writes to pi's session files directly. It changes them only through pi: prompts, `set_session_name`, and so on.

### Session files

pi documents the file format in `docs/session-format.md`. What Ludolph uses:

- The first line is the header: `{"type":"session","id":…,"timestamp":…,"cwd":…}`. `cwd` never changes for a session, so it's the chat's project.
- `session_info` entries carry the title (`name`). The latest one wins.
- `message` entries hold user, assistant, and tool-result messages. An assistant message's `stopReason` tells whether the last turn ended well, failed, or was aborted.
- Files are append-only trees (`id`, `parentId`).

### Index

One row per session file: path, session id, cwd, created time, title, last activity, a preview of the last user message, the last turn's outcome, the archive state, and the byte offset the row was built up to.

Since files are append-only, the engine parses only the bytes after the saved offset. If a file got smaller or its header changed (pi migrates old versions on load), the engine parses it again from the start. On this Mac that's 270 files and 511 MB, so the first full build reads everything once, and later builds read only what's new.

The engine watches the sessions folder with FSEvents, so it also sees sessions that pi run in a terminal changes.

## Projects

A project is a session's `cwd`, as is. Its name is the folder's name. Chats whose cwd is the home folder, or a Ludolph scratch folder, go under **No project**.

A chat started without a project gets its own scratch folder, `~/Library/Application Support/Ludolph/scratch/<date>-<short id>`, the way Zeron does.

Grouping is by the cwd, not the git root, as pi itself groups sessions. None of the cwds in the sessions on this Mac is a subfolder of a repo, so the git root would give the same groups at the cost of a git call per folder.

If a chat's folder no longer exists (3 of the cwds here), pi's RPC mode refuses to open it. Ludolph shows such a chat read-only, and says why.

## pi processes

The engine talks to pi only through RPC mode (`pi --mode rpc`, see pi's `docs/rpc.md`): JSON lines on stdin and stdout, with responses matched by `id`. It splits records only on `\n`, as pi asks.

- **New chat:** start `pi --mode rpc --session-id <uuid>` in the chat's cwd, so Ludolph picks the session id up front. Then send `get_state` to learn the session file. pi writes that file only on the first prompt (checked with pi 1.5.2), so a new chat is a draft in the app until its first message. Starting the process when the composer opens hides most of the 250 ms.
- **Open chat:** reading a transcript needs no process. The engine reads the session file. A process starts only when a chat needs pi: to send a message, change the model, or list skills.
- **Send:** `prompt`. While the agent works, `prompt` with `streamingBehavior: "steer"`.
- **Stop:** `clear_queue`, then `abort`.
- **Model and thinking level:** `get_available_models`, `set_model`, `get_available_thinking_levels`, `set_thinking_level`. The model list is cached, since it's the same for every chat.
- **Skills** (the `$` menu): `get_commands`, keeping those with `source: "skill"`.
- **Images:** sent in the `prompt` command as base64. There's no upload store.
- **Live view:** the engine rebuilds the streaming message from `message_update` deltas and replaces it with `message_end`, as pi's `docs/json.md` describes.
- **Stopping idle processes:** once a chat settles (`agent_settled`) and has no open question, the engine closes its process's stdin after a few idle minutes (a setting, default 5). pi then shuts down cleanly, and the next message starts it again with `--session <file>`. A restart costs about 270 ms even for a large session, while a loaded process holds up to ~300 MB, so stopping idle processes is the biggest memory saving.
- **Crashes:** if pi exits during a turn, the chat shows as failed, and the next message starts pi again.

### Status

| Status | When |
|---|---|
| Working | After `agent_start`, until `agent_settled` |
| Needs you | A dialog `extension_ui_request` (`select`, `confirm`, `input`, `editor`) has no answer yet |
| Failed | The last turn ended with an error, or `auto_retry_end` with `success: false`, or pi exited during a turn |
| Idle | Anything else |

For a chat with no process, the status comes from the index: the outcome of its last turn.

### Questions from the agent

Questions use the shape of Claude Code's `AskUserQuestion` tool, which the phone app and the Companion already render: a list of questions, each with an optional `header`, the `question`, `options` (each with a `label`), and `multiSelect`. An answer is the picked labels, or typed text.

They come from two places:

- **`ask_user`:** Ludolph loads one small extension of its own with `--extension`. It adds an `ask_user` tool with the shape above. pi's RPC dialogs ask one thing at a time and can't pick several options, so the tool doesn't use them. It calls the engine over the [engine socket](#engine-api) and waits for the answer, the way Zeron's MCP bridge (`zeron-mcp.mjs`) calls Zeron. The engine starts each pi with the chat's session id in an environment variable, so the tool's call names its chat.
- **Other extensions' dialogs:** every `extension_ui_request` dialog (`select`, `confirm`, `input`, `editor`) shows as a one-question form. The answer goes back as an `extension_ui_response` (`docs/rpc-extension-ui.md`).

### Sessions open elsewhere

pi doesn't lock session files and doesn't keep them open between writes (checked with `lsof` on running pi processes). If a session is also open in a terminal pi, two writers would append to the same file and fork its history.

The engine knows when its own pi processes write. So when a session file grows and Ludolph has no process for it, another pi is writing. Then:

- The chat shows a banner, "Open in another pi", and its transcript follows the file live.
- Sending is off until the file has been quiet for 2 minutes.
- **Take over** turns sending back on at once, for a terminal pi you know you closed.

A terminal pi that's open but idle writes nothing, so this can't see it. The real fix is a lock in pi itself: we'll propose an advisory lock (`flock`) on the session file upstream, and use it once pi has it.

## Titles

Titles are part of the main flow:

1. Until a chat has a title, the app shows the first words of its first prompt.
2. After a chat's first turn settles, the engine asks the title model for a title, with `pi -p` and no session, as zeron-tools' `rename.rs` does today.
3. It stores the title with `set_session_name` on the chat's live process. That adds a `session_info` entry to pi's file, so pi's own `/resume` shows the same title.
4. If that call fails, it tries again after the next turn.

Renaming a chat by hand goes through the same `set_session_name`. The engine never replaces a title that a person or pi set.

Only 1 of the 270 sessions on this Mac has a `session_info` entry, because Zeron keeps its titles in its own database. See [migration](#migration-from-zeron).

## Archiving

Archiving is part of the main flow, with the rule zeron-tools uses now:

- In each project, the most recent chats stay in the list (a setting, default 10). The rest are archived.
- A chat that's working or waiting for an answer is never archived.
- An archived chat stays searchable and opens and resumes like any other.
- A chat you unarchive stays unarchived until it has new activity. This is the "memory" in `archive.rs`.

The archive state lives in the index. The rule runs when the index changes, not on a timer.

## Turn changes

Each turn's changes are kept, so the app can show what any turn changed, as zeron-tools does now. Zeron keeps one snapshot per chat. Ludolph takes its own with git, without touching your index or your work:

1. When a turn starts, the engine writes the working tree, including untracked files, to a tree object. It uses a temporary index (`GIT_INDEX_FILE`, `git add -A`, `git write-tree`).
2. When the turn settles, it does the same again.
3. It diffs the two trees and saves the patch and its line counts under the entry id of the user message that started the turn. The oldest saved turns are dropped when the total goes over a size budget, as `turns.rs` does now.

Folders that aren't git repos get no turn changes. Two chats working in the same repo at once see each other's changes in their diffs.

## Engine API

The app and the relay link use the same tool layer, as `connect` and the Companion do now.

- **App ↔ engine:** a Unix socket, `~/Library/Application Support/Ludolph/engine.sock`, with mode 0600. It carries JSON lines: calls with an `id` and their replies, plus subscriptions that push changes. These are list changes, a chat's live transcript, and the engine's state. Pushing instead of polling means no 1-second timer while a window is hidden. The `ask_user` extension uses the same socket for its one call, `ask`.
- **Relay ↔ engine:** the relay protocol as it is now (`relay-proto`). Only the phone's tools are allowed, and they're checked in the engine, not on the relay.

### Tools

The phone's tools keep their names and answer shapes, so the phone app needs only small changes:

| Tool | In Ludolph |
|---|---|
| `list_chats` | From the index |
| `list_projects` | The distinct cwds in the index |
| `chat_view` | Read from the session file, plus the live message if the chat is working |
| `send_message` | `prompt`, or a steering `prompt` |
| `create_chat` | Start pi in the project's folder or a scratch folder, then `prompt` |
| `interrupt_chat` | `clear_queue` and `abort` |
| `respond_to_input` | `extension_ui_response` |
| `archive_chat` | Index |
| `turn_changes` | Saved turn changes |
| `composer_info` | Branch (`git`), context use (`get_session_stats`), model and thinking level |
| `set_chat_model` | `set_model`, `set_thinking_level` |
| `list_models` | Cached `get_available_models` |
| `list_skills` | `get_commands` |
| `polish_dictation` | The title model through `pi -p` |
| `list_harnesses` | Removed. There's only pi. |
| `open_zeron` | Removed. If the engine isn't running, the Mac is offline. |

Changes to the phone app: remove the agent picker and the "start Zeron" action, and replace Zeron's name in its text. The relay protocol goes to version 2, and the `Engine` message gets simpler.

## App

The views are shared by both windows. They're today's Companion views, with the Companion's theme, syntax colors, animations, and activity rows, not Zeron's:

- the chat view: transcript, tool-call rows, questions, copy buttons, links
- the turn changes page
- the composer: images, dictation, skills, model, stop
- the chat list rows and status filters

They live in a `ui` crate. Each window gives them its own layout.

- **Main window:** a normal, resizable window. The sidebar has projects, their chats, status filters, and search. The chat is on the right. It also has **Settings**, and an action to open the current chat in the quick window. It shows a Dock icon while open, and the app can be pinned to the Dock.
- **Quick window:** today's Companion: floating and slightly transparent, shown and hidden with the global shortcut (default ⌃⌥Space).
- **Dictation:** Apple's speech recognizer, as now, then cleaned up by the title model.

### Settings

- Title model and its thinking level
- Chats to keep per project
- Minutes before an idle pi process stops
- Global shortcut
- Phone: relay address and link, and notifications even while you're at the Mac
- Open at login
- The pi binary, if it's not on `PATH`

## Phone and relay

The relay, its Kubernetes manifests, and the phone app (`relay/assets`) move into this repo. The phone app keeps its look and flows.

- **Linking a Mac:** the phone's **Add a Mac** gives a link code. You paste it into Ludolph's Settings, rather than running an `install` command.
- **Notifications:** finished, needs you, and failed. As now, they wait until the Mac has been idle for 2 minutes, unless set to always notify.
- **Self-hosting:** the README explains how to run the relay (container image and manifests) and how to create the setup link.

## Performance budget

Targets on an Apple silicon Mac, with no chat working:

| Process | Memory | CPU |
|---|---|---|
| Engine | under 30 MB | about 0% (event-driven, no polling timers) |
| App, windows hidden | under 60 MB | about 0% (no ticks while hidden) |
| App, main window open | under 150 MB | about 0% when nothing changes on screen |
| pi | one process per chat that's working or was used in the last few minutes | — |

How we check:

- `ps -o rss,pcpu` sampled over a minute in each state, on every release.
- Index build time and memory with the 270 sessions on this Mac, cold and warm.
- A soak test: 5 chats working at once for 10 minutes. The engine's memory shouldn't keep growing.

## Repo layout

A Cargo workspace, following fettle where it applies (bundle, signing, release, `make dev`):

```
crates/
  engine/       pi processes, index, titles, archiving, turn changes, tools
  ui/           shared Rune views
  relay/        the relay server and the phone app
  relay-proto/  the Mac ↔ relay messages
  json-file/    atomic JSON files
src/main.rs     the `ludolph` binary: `engine`, the app, and `install`/`uninstall` for the launch agent
```

## Migration from Zeron

- **Chats:** nothing to do. Zeron runs pi with session files under `~/.pi/agent/sessions`, so Ludolph's index finds them.
- **Titles and archive state:** these live in Zeron's database. A one-time import runs while Zeron still runs, through zeron-tools' IPC client. It lists chats with their titles and archive state. It then maps each to its session file through `~/.pi/agent/zeron-sessions/<hash>.json`, which holds `{sessionId, sessionFile}`. That the hash is derived from the Zeron chat id is still to be checked. The import writes titles with `set_session_name` and archive state to the index. If the import is skipped, titles fall back to first words, and archiving starts from scratch.
- **Turn changes:** zeron-tools' saved turns (`~/.zeron/turn-snapshots` and its own store) aren't carried over.

## Milestones

1. **Engine:** the index, pi processes, status, titles, archiving, turn changes, the socket API, and the relay link. The phone works against it, and Zeron can be removed.
2. **Quick window:** the Companion ported to the engine's socket, with the views moved into `ui`.
3. **Main window:** the sidebar, Dock icon, and app bundle, signed like fettle.
4. **Settings:** including linking a phone.
5. **Migration import**, then the first public release.

## Alternatives considered

- **One process for the engine and the app.** It would save a few MB, but quitting or updating the app would stop every agent and cut off the phone.
- **Embedding pi through its TypeScript SDK.** It would put a JavaScript runtime in the engine and tie it to pi's internals. RPC is pi's documented interface for other languages, and it keeps each chat in its own process.
- **Ludolph's own transcript store, as Zeron has.** It would be a second copy of every chat, and it would drift from pi's files. pi's files already have everything the views need.
- **Polling instead of FSEvents and pushed updates.** Polling costs CPU while nothing happens, which is what the budget rules out.
- **Grouping projects by git root.** It gave the same groups as the cwd on this Mac's sessions, for a git call per folder.
