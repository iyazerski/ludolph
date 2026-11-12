# Ludolph design

Status: accepted.

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
- Files are append-only trees (`id`, `parentId`). pi can still rewrite a whole file, for example when it migrates an old version on load.

The file order mixes branches: going back with `/tree` leaves the old branch in the file. So the transcript is never "every `message` entry". It's the path from the current leaf (the last entry appended) back to the root, following `parentId`. Compaction doesn't hide history: the transcript shows every message on the path, with the compaction summary as a row where it happened. `branch_summary` and displayed `custom_message` entries are rows too.

A fork (`/fork`, `/clone`) copies entries, with their ids, into a new file. So anything keyed by an entry id is keyed by session id and entry id together.

### Index

One row per session file, in two parts:

- **Derived from the file**, and rebuilt from it at any time: path, session id, cwd, created time, title, last activity, a preview of the last user message, the last turn's outcome, the current leaf, and the byte offset the row was built up to.
- **Ludolph's own**, which can't be rebuilt: the archive state (see [the chat list and archiving](#the-chat-list-and-archiving)) and a title imported from Zeron that isn't in pi's file yet.

Since files are append-only, the engine parses only the bytes after the saved offset, and only complete lines. A partial last line waits for the next read. If a file got smaller or its header changed, the engine parses it again from the start. The parse streams line by line and keeps only the index fields, so a 45 MB file never sits in memory whole. On this Mac that's 270 files and 511 MB, so the first build reads everything once, and later builds read only what's new.

The engine watches the sessions folder with FSEvents, so it also sees sessions that a terminal pi changes. FSEvents can drop or merge events, so it's a hint, not a log: on start, and whenever FSEvents reports dropped events, the engine compares every file's size and modification time with the index.

## Projects

A project is a session's `cwd`, as is. Its name is the folder's name. Chats whose cwd is the home folder, or a Ludolph scratch folder, go under **No project**, which is one group in the list.

A chat started without a project gets its own scratch folder, `~/Library/Application Support/Ludolph/scratch/<date>-<short id>`, the way Zeron does.

Grouping is by the cwd, not the git root, as pi itself groups sessions. None of the cwds in the sessions on this Mac is a subfolder of a repo, so the git root would give the same groups at the cost of a git call per folder.

If a chat's folder no longer exists (3 of the cwds here), pi's RPC mode refuses to open it. Ludolph shows such a chat read-only, and says why.

## pi processes

The engine talks to pi only through RPC mode (`pi --mode rpc`, see pi's `docs/rpc.md`): JSON lines on stdin and stdout, with responses matched by `id`. It splits records only on `\n`, as pi asks.

- **New chat:** start `pi --mode rpc --session-id <uuid>` in the chat's cwd, so Ludolph picks the session id up front. Then send `get_state` to learn the session file. pi writes that file only on the first prompt (checked with pi 1.5.2), so a new chat is a draft in the app until its first message. Starting the process when the composer opens hides most of the 250 ms.
- **Open chat:** reading a transcript needs no process. The engine reads the session file. A process starts only when a chat needs pi: to send a message, change the model, or list skills.
- **Send:** `prompt`. While the agent works, `prompt` with `streamingBehavior: "steer"`.
- **Stop:** `clear_queue`, then `abort`.
- **Model and thinking level:** `get_available_models`, `set_model`, `get_available_thinking_levels`, `set_thinking_level`. Project-local extensions can add providers, so the model list is cached per project and refreshed whenever a process in that project starts. Thinking levels come from the chat's own process, for its current model.
- **Skills** (the `$` menu): `get_commands`, keeping those with `source: "skill"`.
- **Images:** sent in the `prompt` command as base64. There's no upload store.
- **Live view:** see [live view and the file](#live-view-and-the-file).
- **Stopping idle processes:** once a chat settles (`agent_settled`) and has no open question, the engine closes its process's stdin after a few idle minutes (a setting, default 5). pi then shuts down cleanly, and the next message starts it again with `--session <file>`. A restart costs about 270 ms even for a large session, while a loaded process holds up to ~300 MB, so stopping idle processes is the biggest memory saving.
- **Crashes:** if pi exits during a run, the chat shows as interrupted, and the next message starts pi again.
- **Engine restarts:** the engine writes a small runtime file, `runtime.json`, with each pi process's pid, its session, and whether a run is in flight. On start, it stops any pi it finds there that's still running, before any new pi may write to that session. Its stdin closed with the old engine, so it gets SIGTERM, and its process group gets SIGKILL if it's still running after 10 s. The file keeps each process's start time, so a reused pid is left alone. A run that was in flight shows as interrupted. It doesn't count as finished, since the file alone can't tell.
- **Shutdown:** on SIGTERM, the engine closes every pi's stdin and gives them 10 s to finish before it kills what's left. Each pi runs in its own process group, so its tool processes go with it.

### Live view and the file

Two sources describe a working chat: pi's events, and the file. pi sends `message_end` before it appends the entry to the file (`agent-session.js`, pi 1.5.2), and its events carry no entry ids. So:

1. During a run, the engine keeps a live tail built from the events: messages from `message_end`, and the streaming one from its `message_update` deltas, replaced by its final block content as each block ends.
2. The transcript is the file's path to its leaf, followed by the tail's messages that the file doesn't have yet. A message and its entry share their role, their `timestamp` and, for a tool result, its `toolCallId`, which is how they're matched. A turn that's partly written shows as one message.
3. When the run settles, the file has every message, and the tail is dropped.

A message's id is its first entry's id. A turn that's only in the tail has a `live-<n>` id until pi writes its first entry.

Every change bumps the chat's revision, and subscribers get a notice with it. The notices coalesce: a client that falls behind sees only the latest, and then reads the view again, so a reconnect needs nothing special. The view is cheap to read again, since each open file is scanned once, then only for what pi appended, and only the lines of the page asked for are parsed.

### Status

| Status | When |
|---|---|
| Working | After `agent_start`, until `agent_settled` |
| Needs you | A dialog `extension_ui_request` (`select`, `confirm`, `input`, `editor`) has no answer yet |
| Failed | The last turn ended with an error, or `auto_retry_end` with `success: false` |
| Interrupted | pi or the engine stopped during a run |
| Idle | Anything else |

For a chat with no process, the status comes from the index: the outcome of its last turn.

### Questions from the agent

Ludolph adds no question tool of its own. The questions it shows are extensions' dialogs: every `extension_ui_request` dialog (`select`, `confirm`, `input`, `editor`) waits for an answer, so a run would hang without one. Each shows as a one-question form in the shape the phone app and the Companion already render: an optional `header`, the `question`, `options` as strings, `multiline` and `prefill`. A `select` offers its options, a `confirm` offers Yes and No under its title, and an `editor` is multi-line with its prefilled text. An answer is the picked option, or typed text. It goes back as an `extension_ui_response` (`docs/rpc-extension-ui.md`).

Each pi process keeps its open questions, oldest first, and the chat shows the oldest. `respond_to_input` looks the id up and answers it. An id is answered once. Later answers, and answers to a question from a process that's gone, are refused. A question leaves when it's answered, when its dialog's `timeout` runs out (pi resolves it itself and doesn't say so), when the run is interrupted (the engine dismisses it first), or when its pi exits. A pi with an open question isn't stopped for being idle, since an extension command can wait on it outside a run.

### Sessions open elsewhere

pi doesn't lock session files and doesn't keep them open between writes (checked with `lsof` on running pi processes). If a session is also open in a terminal pi, two writers would append to the same file and fork its history.

A pi loads its session file once and then only appends (`session-manager.js`, pi 1.5.2). So a second pi doesn't just add lines. It also writes from an old leaf, and the file branches.

Every run starts with a user message, and pi writes the `timestamp` of the message it reports into the file. So each chat remembers the timestamps of the user messages its own pi reported. When the newest user message in a changed file isn't one of them, another pi wrote it. pi may write the file before it reports the message, so while the chat has its own pi, the engine gives that pi a second to report it first. Then:

- The engine stops its own pi for that chat, if it has one, since that pi's view of the file is now stale.
- The chat shows a banner, "Open in another pi", and its transcript follows the file live.
- Sending stays off until you press **Take over**. It never turns back on by itself, since a terminal pi can run a long tool call without writing anything. Take over starts a fresh pi, which loads the file as it is then.

This also marks chats that Ludolph never opened, when a terminal pi sends a prompt in them. The mark lives in memory, so it's cleared when the engine restarts.

A terminal pi that's open but idle writes nothing, so this can't see it. The real fix is a lock in pi itself: we'll propose an advisory lock (`flock`) on the session file upstream, and use it once pi has it.

## Titles

Titles are part of the main flow:

1. Until a chat has a title, the app shows the first words of its first prompt.
2. As soon as Ludolph sends a message to a chat without a title, the engine asks the title model for one, while the run goes on. The model sees the chat's first prompt, or the message itself for a new chat, clipped to 2,000 characters. A new chat has its title about 3 seconds after you send. Messages an extension command handles (like `/name`) don't ask.
3. It runs `pi -p` the way zeron-tools' `pi.rs` does: in a temporary folder, with no session, tools, extensions, skills, prompt templates, context files, or MCP, with a 60-second limit, and with at most two running at once.
4. When the title comes back, the engine asks the chat's pi for its state. If the session got a name in the meantime, from you or from pi, the new title is dropped. Otherwise it stores the title with `set_session_name`. pi keeps the name until it writes the session file, then adds it as a `session_info` entry, so pi's own `/resume` shows the same title. The list shows the title at once, even before the file exists.
5. If any step fails, or the chat's pi stopped before the title came back, the next message tries again.

The title model is `LUDOLPH_TITLE_MODEL` at `LUDOLPH_TITLE_THINKING` (default `low`) until there are settings, or pi's default model if that's unset. A small, fast model keeps the delay short.

Renaming a chat by hand goes through the same `set_session_name`. The engine never replaces a title that a person or pi set. Calls that change one chat (title, rename, model) go through one queue per chat, so they can't overtake each other.

Only 1 of the 270 sessions on this Mac has a `session_info` entry, because Zeron keeps its titles in its own database. See [migration](#migration-from-zeron).

## The chat list and archiving

Nothing is archived automatically. Zeron's rule (archive all but each project's 10 newest chats) existed because Zeron slowed down with many chats. Ludolph's index keeps every summary in memory, so a long list costs nothing to produce. Keeping it short is the list's job:

- Each project, and **No project**, shows its 10 newest chats and its total. **Show more** adds 10. The windows remember what's expanded in memory only, so a restart shows 10 again.
- `list_chat_groups` returns every group with its newest chats, and takes a higher limit for each expanded group. A window asks for the whole list again on each list notice, so there are no offsets that drift when chats change order.
- The phone keeps its flat `list_chats`, the 200 newest chats.

Archiving is something you do, to put away a chat you're done with:

- An archived chat leaves the groups and the phone's list. `list_chats` with `archived: true` lists the archived chats, for an **Archived** section that pages the same way.
- An archived chat opens and resumes like any other. When it has new activity, it's no longer archived.

"Activity" means a new user or assistant message. A new title doesn't count, so titling neither moves a chat up the list nor brings it back from the archive.

The archive state is Ludolph's own part of the index: each archived chat's id, with its last activity at that moment. The index drops it when the chat's activity is newer.

## Turn changes

The app shows what each run changed, as zeron-tools does now. A run is everything from the prompt that starts it until pi settles, including any steering messages sent in between. pi's own `turn_start` is one assistant response, which is too small a unit, and the engine can't pause pi to take a snapshot in the middle of a run.

The engine takes its snapshots with git, without touching your repo:

1. **Before** it forwards a prompt to an idle chat, it snapshots the working tree, and only then sends the prompt. So no tool can change a file before the snapshot.
2. When the run settles, it snapshots again. A new prompt waits for that snapshot.
3. It diffs the two snapshots and saves the patch and its line counts under the session id and the entry id of the user message that started the run.

A snapshot copies the repo's index to a temporary file and runs `git add -A` and `git write-tree` against that copy (`GIT_INDEX_FILE`). New file contents go to Ludolph's own object store (`GIT_OBJECT_DIRECTORY`), with the repo's objects as an alternate (`GIT_ALTERNATE_OBJECT_DIRECTORIES`). So the repo gets no new objects, your index and staged changes stay as they are, and `.gitignore` is respected, which keeps ignored secrets out. This was checked on a test repo, and a snapshot of zeron-tools took 44 ms.

Limits:

- A snapshot that takes more than 10 s, or a new file over 10 MB, skips that run's changes and says so.
- Binary files show as changed, without content.
- Ludolph's object store is pruned with the saved patches: when they go over the size budget (as in `turns.rs` now), the oldest go first, then `git prune` removes what no saved run or running run still needs.

Folders that aren't git repos get no run changes. Two chats working in the same repo at once see each other's changes in their diffs.

## Engine API

The app and the relay link use the same tool layer, as `connect` and the Companion do now.

- **App ↔ engine:** a Unix socket, `~/Library/Application Support/Ludolph/engine.sock`, with mode 0600. It carries JSON lines: calls with an `id` and their replies, plus subscriptions that push a notice when something changes: the chat list, one chat's view, and later the engine's state. The client then reads what changed with a call, like `chat_view`, which can answer "unchanged" for the version the client already shows. Pushing instead of polling means no 1-second timer while a window is hidden. Notices that wait merge into one, so a slow client gets no backlog.
- **Relay ↔ engine:** the relay protocol as it is now (`relay-proto`). Only the phone's tools are allowed, and they're checked in the engine, not on the relay.

### Tools

The phone's tools keep their names and answer shapes, so the phone app needs only small changes:

| Tool | In Ludolph |
|---|---|
| `list_chats` | From the index |
| `list_chat_groups` | From the index, by project |
| `list_projects` | The distinct cwds in the index |
| `chat_view` | The [transcript](#live-view-and-the-file)'s latest messages, a page at a time. Long tool output and images load on request. |
| `send_message` | `prompt`, or a steering `prompt` |
| `create_chat` | Start pi in the project's folder or a scratch folder, then `prompt` |
| `interrupt_chat` | `clear_queue` and `abort` |
| `respond_to_input` | Answers the [open question](#questions-from-the-agent) with that id, as an `extension_ui_response` |
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
- **Notifications:** finished, needs you, failed, and interrupted. As now, a notice is dropped if the Mac's keyboard and mouse were used in the last 2 minutes, unless set to always notify. It isn't sent later.
- **Self-hosting:** the README explains how to run the relay (container image and manifests) and how to create the setup link.

### Trust

The relay sees every call and answer in plain text, and it can call `send_message`. An agent can run commands, so whoever controls the relay can run code on your Mac. The engine allowing only the phone's tools doesn't change that. So the relay is trusted like your Mac. Run it yourself, keep passkey sign-in, origin checks, and revoking a Mac, and keep the Mac's relay token in the Keychain. End-to-end encryption between the phone and the Mac would remove that trust. It's out of scope for now.

## Performance budget

Targets on an Apple silicon Mac, with no chat working:

| Process | Memory | CPU |
|---|---|---|
| Engine | under 30 MB | about 0% (event-driven, no polling timers) |
| App, windows hidden | under 60 MB | about 0% (no ticks while hidden) |
| App, main window open | under 150 MB | about 0% when nothing changes on screen |
| pi | one process per chat that's working or was used in the last few minutes | — |

What keeps the engine inside its budget:

- Index parsing streams line by line (see [index](#index)).
- Transcripts load in pages, and long tool output and images load only when shown.
- Subscriber queues are bounded (see [engine API](#engine-api)).
- pi's stdout is read all the time, as pi asks, but the engine keeps only each chat's live tail.

How we check:

- `ps -o rss,pcpu` sampled over a minute in each state, on every release.
- Index build time and memory with the 270 sessions on this Mac, cold and warm.
- A soak test: 5 chats working at once for 10 minutes, one of them streaming a long answer, with a phone that stops reading. The engine's memory shouldn't keep growing.

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
- **Zeron's scratch folders:** chats Zeron started without a project ran in `~/.zeron/scratch/<date>-<id>`, and their sessions keep that cwd. Without help, each shows as a project of its own. The import decides how they're grouped.
- **Moved chats:** when Zeron moved a chat to a project, it left a copy of the session file under the old folder, with the same session id. The index lists a session once, by the copy written last.
- **Titles and archive state:** these live in Zeron's database. A one-time import runs while Zeron still runs, through zeron-tools' IPC client. It lists chats with their titles and archive state. It then maps each to its session file through `~/.pi/agent/zeron-sessions/<hash>.json`, which holds `{sessionId, sessionFile}`. That the hash is derived from the Zeron chat id is still to be checked. The import writes titles and archive state to Ludolph's part of the index only. It starts no pi, so it's safe while Zeron still runs. Each imported title moves into pi's file with `set_session_name` the next time its chat runs in Ludolph. If the import is skipped, titles fall back to first words, and nothing is archived.
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
