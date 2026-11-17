# Architecture

How Ludolph fits together, and the rules that hold across modules. Module details live in the code. pi behavior Ludolph depends on is in [pi.md](pi.md), the phone link in [relay.md](relay.md), and resource limits in [performance.md](performance.md).

## Scope

A Mac app for pi only, and an alternative to pi's terminal UI: it aims for the same features and controls, through pi's RPC mode. Chats grouped by project, a main window, a quick window on a global shortcut, and a phone web app through a self-hosted relay. No other agents, Ludolph accounts, sync, terminals, or worktrees.

Linux, and Windows with the engine in WSL, get a reduced app: what needs macOS (dictation, browser and computer use, Open at login, the Keychain, installing updates, battery and idle checks for the phone) is left out rather than rebuilt. Windows runs only the app, because pi and the tools it runs belong in WSL.

## Processes

```
 phone ──HTTPS──▶ relay ◀──WebSocket── engine ◀──socket── app (main + quick window)
                                         │
                                         ├── pi --mode rpc  (one per active chat)
                                         ├── pi -p          (titles, dictation cleanup)
                                         └── git            (turn changes)
```

- **Engine** (`ludolph engine`): a launchd job with no UI. It owns the pi processes, the index, settings, turn changes, and the relay link. It's a separate process so agents and the phone keep working when the app quits, crashes, or updates.
  - Its plist is `engine.plist` in the state folder, not in `~/Library/LaunchAgents`. The app loads it for the login session on each start. So nothing starts at login unless **Open at login** is on.
  - launchd's environment is bare, so the engine reads the login shell's (`$SHELL -i -l -c env`, 10 s limit), as VS Code does. Without that, pi wouldn't see the tools, API keys, and proxy settings a terminal does.
- **App**: one process for both windows. It's an accessory app, with a Dock icon only while the main window is open. `app.lock` keeps a second copy from fighting over the shortcut. The login item runs `open -g -a Ludolph.app --args --hidden` through LaunchServices, so opening the app later reaches the running copy.
- **Relay** (`crates/relay`): a small server the user hosts. It routes sealed messages it can't read.

Both processes run from the same binary. A development run (`LUDOLPH_DIR` set, or outside a bundle) leaves the installed engine's job and login item alone.

Outside macOS there's no launchd. The app runs `ludolph connect`, which joins its stdin and stdout to `engine.sock`, and first starts the engine in its own process group if nothing answers. `engine.lock` keeps a second engine from starting. The Windows app runs the same command in the default WSL distribution with `wsl.exe`, so it needs no open port, and the socket's 0600 mode stays the only access check. Paths in the API are the engine's, so the Windows app maps folders it picks into WSL with `wslpath`, refuses folders in another distribution, and doesn't open files inside WSL.

## Sources of truth

| Data | Where | Owner |
|---|---|---|
| Transcript, cwd, model, thinking level, title | pi's session files, `~/.pi/agent/sessions/**.jsonl` | pi |
| Index: one row per session | `index.json` in the state folder | engine |
| Turn changes | `changes/<session id>/` in the state folder | engine |
| Settings | `settings.json` in the state folder | engine; the app changes them through it |
| UI state (seen turns, window frames, accent, shortcut) | `ui.json` in the state folder | app |
| Attached images | `~/.pi/agent/attachments/<session id>/` | engine |

The state folder is `~/Library/Application Support/Ludolph` on macOS, `$XDG_STATE_HOME/ludolph` (`~/.local/state/ludolph`) on Linux, and `%LOCALAPPDATA%\Ludolph` for the Windows app's `ui.json`.

pi's session files are the only chat history. Ludolph never keeps a second transcript store. Its index is a cache that can be rebuilt from the files, except for the "interrupted by the engine" mark.

Ludolph changes sessions only through pi (`prompt`, `set_session_name`, and so on). Moving a chat is the one exception, since pi has no move RPC: with the chat's pi stopped, the engine copies the file into the destination folder with a new header `cwd`, then removes the old copies. It never overwrites a destination file.

### Reading session files

- The transcript is the path from the current leaf (the last entry appended) back to the root, following `parentId`. It's never "every `message` entry", because `/tree` leaves old branches in the file.
- Anything keyed by an entry id is keyed by session id and entry id together, because forks copy entries with their ids.
- Files are append-only, so the index parses only complete lines after its saved offset. A file that got smaller or changed its header is parsed again from the start. Parsing streams line by line, so a large file never sits in memory whole.
- FSEvents and inotify are hints, not logs. On start, and whenever they report dropped events, the engine compares every file's size and mtime with the index. inotify also misses files written into a new folder before its watch begins, so a new folder does the same.

### Projects

A project is a session's `cwd`, as is, the way pi groups sessions. The git root gave the same groups on real data, but it costs a git call per folder. Chats in the home folder or a Ludolph scratch folder go under **No project**. A chat whose folder is gone is read-only, because pi's RPC mode refuses to open it.

## pi processes

- A process starts only when a chat needs pi: sending, changing the model, or listing skills. Reading a transcript reads the file.
- A new chat starts pi with `--session-id <uuid>`, so Ludolph knows the id up front. It stays a draft until the first prompt, because pi writes the file only then.
- An idle chat's pi is stopped after 5 minutes by closing its stdin, unless a question is open. A restart costs about 270 ms, while a loaded pi holds up to ~300 MB, so this is the biggest memory saving.
- A pi that crashes takes its process group with it, so its tools stop changing files. A clean exit leaves background jobs alone, as a terminal pi does.
- `runtime.json` records each pi's pid, start time, session, and whether a run is in flight. After an engine restart, the engine kills those processes before any new pi can write to the session, and marks in-flight runs as interrupted.
- Calls that change one chat (title, rename, model) go through one queue per chat, so they can't overtake each other.

### Live view

pi reports messages before writing them, and its events carry no entry ids. So during a run the engine keeps a live tail built from events. The transcript is the file's path, then the tail messages the file doesn't have yet, matched by role, `timestamp`, and `toolCallId`. The tail is dropped when the run settles. It keeps no tool output or image data.

Clients subscribe to a chat's feed. The first frame resets the page. Later frames carry only new or changed messages, at most every 150 ms (200 ms over the relay). Changes in between merge, so a slow client gets the latest state, not a backlog. Pages are anchored on their first message, not a count from the end, so loaded history stays in place as new messages come.

### Sessions open elsewhere

pi doesn't lock session files. A second pi appends from its own stale leaf, so the file branches. The engine detects this by remembering the timestamps of user messages its own pi reported. A newest user message it didn't report was written by another pi. Then the engine stops its own pi, shows **Open in another pi**, and disables sending until **Take over**. Sending never turns back on by itself, since a terminal pi can run a long tool call without writing anything.

An idle terminal pi writes nothing, so this can't catch it. The real fix is an advisory lock in pi itself.

### Titles

Titles come from the sidecar model through `pi -p` with no session, tools, extensions, skills, or context files. The engine stores a title with `set_session_name` only if the session still has no name. It never replaces a title set by a person or by pi. Setting a title doesn't count as activity, so it doesn't move a chat up the list.

### Agent tools

Ludolph's pi extensions live in `extensions/` and ship as one JS file each in `Contents/Resources/pi`. The engine passes the ones turned on in Settings to each chat's pi with `--extension`, and the GUI and reviewer selections through environment variables. So a settings change reaches a chat when its pi next starts.

- Ludolph's extensions stay out of pi's own setup, so a terminal pi doesn't load them. Settings that are pi's own, such as sign-in or which skills load, change pi's setup as pi's terminal UI would, so a terminal pi sees them too.
- The sidecar and the second-opinion reviewer load no extensions. A GUI subagent loads only `gui.js` in its role mode, which adds no subagent tools. So no child can start another.
- Browser and computer use install browse or Cua Driver only after the user confirms. Both run their MCP server inside a subagent, so a chat's pi doesn't start them until a task needs them.

## Turn changes

A run, not pi's `turn_start`, is the unit of a diff: everything from the prompt that starts it until pi settles, including steering messages. The engine snapshots the working tree before it forwards the prompt and again when the run settles, so no tool can change a file before the first snapshot.

Snapshots must never change the user's repository. They use a temporary copy of the index (`GIT_INDEX_FILE`) and a private object store with the repo's objects as an alternate. They add only the paths `git status` names, so a large repo costs about one `git status`. `.gitignore` is respected, which keeps ignored secrets out. Changed blobs are copied into the chat's own store, so diffs survive commits and later edits.

## Engine API

The app talks to the engine over a Unix socket (`engine.sock`, mode 0600) with JSON lines: calls with an `id`, and subscriptions that push changes. Pushing instead of polling keeps hidden windows at 0% CPU. The phone calls the same tools through the relay, but the engine allows only the phone's subset, and it checks that itself, not the relay.

## Releases

Each push to `main` that changes the app publishes `vX.Y.n` (`.github/workflows/release.yml`). Every release is signed with the same self-signed certificate (`MACOS_CERT_P12`), so macOS keeps the microphone, speech, and Keychain grants across updates. The app isn't notarized. Release notes use the original commit subjects of each merged PR. An API failure stops the release rather than leaving changes out of the notes.

## Rejected alternatives

- **One process for the engine and the app.** It saves a few MB, but quitting or updating the app would stop every agent and cut off the phone.
- **Embedding pi through its TypeScript SDK.** It puts a JavaScript runtime in the engine and ties it to pi's internals. RPC is pi's documented interface.
- **Ludolph's own transcript store.** It would be a second copy of every chat, and it would drift from pi's files.
- **Polling instead of FSEvents and pushed updates.** Polling costs CPU while nothing happens.
- **Grouping projects by git root.** See [Projects](#projects).
