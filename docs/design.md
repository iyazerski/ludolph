# Ludolph design

Status: accepted.

## Goal

Ludolph is a Mac app for [pi](https://pi.dev) coding agents. It replaces two things: Zeron (its engine and app) and zeron-tools (`connect` and the Companion). It should stay simple, and it should be light on CPU and memory.

It keeps what zeron-tools has today, and nothing more from Zeron:

- chats grouped by project, with live transcripts, tool-call rows, each turn's changes, and the composer (images, dictation, skills, model and thinking level, stop, rename, delete)
- questions from the agent, answered in the app
- titles and deleting as part of the main flow, not as workarounds
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

- **Engine** (`ludolph engine`): a launchd job. It restarts after a crash, and keeps running when the app quits, so agents and the phone carry on. It owns the pi processes, the index, titles, deleting chats, turn changes, and the relay link. It has no UI.
  - Nothing of Ludolph's starts at login unless you turn on **Open at login**. So the engine's plist isn't in `~/Library/LaunchAgents`: it's `engine.plist` in the state folder, and each time `Ludolph.app` starts, it loads that job for the rest of the login session. It reloads it if the app moved, and starts the engine if it stopped, as after an update. The engine logs to `~/Library/Logs/Ludolph/engine.log`. `ludolph uninstall` stops it and removes the app's login item.
  - launchd's environment is bare, so the engine starts by asking your login shell for its own, like VS Code does. It runs `env` through `-i -l -c` with your shell from the user database, with a 10-second limit, and keeps launchd's if that fails. It takes about 120 ms on this Mac. pi and its tools then find what a terminal would: tools you install later, and the API keys and proxy settings your rc files export.
- **App** (`Ludolph.app`): one UI process with both windows. With **Open at login** on, it starts at login, so the global shortcut always works. It shows a Dock icon only while the main window is open, and runs as an accessory app otherwise. Rune supports several windows in one app and switches between the regular and accessory activation policies (checked in Rune's source at the pinned revision).
  - `ludolph` with no arguments runs it and opens the main window, which is what opening `Ludolph.app` does. `--hidden` starts it with no window. It takes `app.lock` in the state folder, so a second copy quits instead of fighting over the shortcut.
  - The login item is a launch agent, `~/Library/LaunchAgents/dev.iyazerski.ludolph.plist`. The engine owns the `openAtLogin` setting, off by default, so it writes the item when the setting turns on, removes it when it turns off, and rewrites it on start if the app moved. The item runs `open -g -a Ludolph.app --args --hidden`: through LaunchServices, so opening the app later reaches the running copy, which opens the main window. It isn't loaded at once, since that would start the app again.
  - `LSUIElement` keeps the Dock icon off until the main window shows. A development run (`LUDOLPH_DIR` set, or outside a bundle) leaves the engine's job and the login item alone, and you run its engine with `make engine`.
  - `scripts/bundle.sh` builds the bundle, and `make install` puts it in `/Applications`. It signs with a self-signed identity kept in its own keychain (`scripts/local-identity.sh`), so macOS keeps the microphone and speech grants across installs, as fettle does for `make dev`. Releases, signed with one certificate from CI, come with the first public release.
- **Relay** (`crates/relay`): a small server you host. It started as zeron-tools' `zeron-relay`.

Why the engine is its own process: agents and the phone keep working when the app is quit, crashes, or updates. That's the reason Zeron's engine was moved to a service in zeron-tools 0.19.

Both processes run from the same binary inside `Ludolph.app`.

## Sources of truth

| Data | Where | Owner |
|---|---|---|
| Transcript, cwd, model, thinking level, title | pi's session files, `~/.pi/agent/sessions/**.jsonl` | pi |
| Index: one row per session | `~/Library/Application Support/Ludolph/index.json` | engine |
| Turn changes | `~/Library/Application Support/Ludolph/turns/<session id>/<entry id>.json` | engine |
| Settings | `~/Library/Application Support/Ludolph/settings.json` | engine; the app changes them through it |
| UI state (seen turns, folded projects, window frames, last picks) | `~/Library/Application Support/Ludolph/ui.json` | app |
| Pasted images, which sent messages link to | `~/Library/Application Support/Ludolph/uploads/` | app |

Ludolph changes session contents through pi: prompts, `set_session_name`, and so on. Moving a chat is the one exception because pi has no move RPC: with its pi stopped, the engine streams a copy into the destination's session folder, changing only the header's `cwd`, then removes the old copies. The session id, history, and saved turn changes stay the same. Busy chats, queued messages, and sessions open in another pi must be resolved before moving. A destination file is never overwritten.

### Session files

pi documents the file format in `docs/session-format.md`. What Ludolph uses:

- The first line is the header: `{"type":"session","id":…,"timestamp":…,"cwd":…}`. `cwd` is the chat's project. Moving a chat changes it before pi next resumes the session.
- `session_info` entries carry the title (`name`). The latest one wins.
- `message` entries hold user, assistant, and tool-result messages. An assistant message's `stopReason` tells whether the last turn ended well, failed, or was aborted.
- Files are append-only trees (`id`, `parentId`). pi can still rewrite a whole file, for example when it migrates an old version on load.

The file order mixes branches: going back with `/tree` leaves the old branch in the file. So the transcript is never "every `message` entry". It's the path from the current leaf (the last entry appended) back to the root, following `parentId`. Compaction doesn't hide history: the transcript shows every message on the path, with the compaction summary as a row where it happened. `branch_summary` and displayed `custom_message` entries are rows too.

A fork (`/fork`, `/clone`) copies entries, with their ids, into a new file. So anything keyed by an entry id is keyed by session id and entry id together.

### Index

One row per session file, in two parts:

- **Derived from the file**, and rebuilt from it at any time: path, session id, cwd, created time, title, last activity, a preview of the last user message, the last turn's outcome, the current leaf, and the byte offset the row was built up to.
- **Ludolph's own**, which can't be rebuilt: which chats the engine cut short (see [engine restarts](#pi-processes)).

Since files are append-only, the engine parses only the bytes after the saved offset, and only complete lines. A partial last line waits for the next read. If a file got smaller or its header changed, the engine parses it again from the start. The parse streams line by line and keeps only the index fields, so a 45 MB file never sits in memory whole. On this Mac that's 270 files and 511 MB, so the first build reads everything once, and later builds read only what's new.

The engine watches pi's agent folder with FSEvents, filtering session changes for the index and model/auth/settings changes for the catalog. It also sees sessions that a terminal pi changes. FSEvents can drop or merge events, so it's a hint, not a log: on start, and whenever FSEvents reports dropped events, the engine compares every file's size and modification time with the index.

## Projects

A project is a session's `cwd`, as is. Its name is the folder's name. Chats whose cwd is the home folder, or a Ludolph scratch folder, go under **No project**, which is one group in the list.

A chat started without a project gets its own scratch folder, `~/Library/Application Support/Ludolph/scratch/<date>-<short id>`, the way Zeron does.

Grouping is by the cwd, not the git root, as pi itself groups sessions. None of the cwds in the sessions on this Mac is a subfolder of a repo, so the git root would give the same groups at the cost of a git call per folder.

If a chat's folder no longer exists (3 of the cwds here), pi's RPC mode refuses to open it. Ludolph shows such a chat read-only, and says why.

## pi processes

The engine talks to pi only through RPC mode (`pi --mode rpc`, see pi's `docs/rpc.md`): JSON lines on stdin and stdout, with responses matched by `id`. It splits records only on `\n`, as pi asks.

- **New chat:** start `pi --mode rpc --session-id <uuid>` in the chat's cwd, so Ludolph picks the session id up front. Then send `get_state` to learn the session file. pi writes that file only on the first prompt (checked with pi 1.5.2), so a new chat is a draft in the app until its first message. Starting the process when the composer opens hides most of the 250 ms. A draft whose pi stops idle before writing the file is dropped, since there's nothing to show. One whose pi dies mid-run stays, interrupted, so its message can be sent again with the same session id.
- **Open chat:** reading a transcript needs no process. The engine reads the session file. A process starts only when a chat needs pi: to send a message, change the model, or list skills.
- **Send:** `prompt`. While the agent works, `sendBehavior` defaults to `queue`: hold the message in the engine, then send one per cleanly finished run as a new turn. Waiting messages show above either composer, on Mac and phone. On Mac each has Edit, Delete and Steer controls. Edit puts its text in the main composer with Save and Cancel, restoring the unsent draft and its attachments afterward. Steer sends just that message to the current run, regardless of the default send behavior. They have stable ids, keep their images when edited, and stay out of the transcript until pi accepts them. Each chat is limited to 32 waiting messages or 12 MiB, including images. Like pi’s queue, these unsent drafts live only for the engine’s life. After a failed run the queue pauses, with **Send next** to retry. Stop clears it. With `sendBehavior: "steer"`, hand the prompt to pi immediately with `streamingBehavior: "steer"`; pi owns delivery from that point and cannot recall or edit just one steering message.
- **Stop:** `clear_queue`, then `abort`.
- **Model picker:** both Mac windows use the same searchable inline combobox as Settings, anchored above the composer. A separate adjacent dropdown picks a supported reasoning level. Neither opens a dialog or covers the transcript.
- **Model and thinking level:** `set_model` and `set_thinking_level` on the chat's process. pi records both in the session file (`model_change`, `thinking_level_change`), and keeps the level within what the model supports. The model list comes from `get_available_models` on a short-lived `pi --mode rpc --no-session` in the engine's folder, cached for up to 10 minutes, but invalidated as soon as `auth.json`, `models.json`, or `settings.json` changes. File identities and write times are compared without reading credentials. Visible Mac windows subscribe to model changes; reopening a window refreshes too. The phone refreshes while visible at most once every 10 seconds and immediately on opening its picker. No chat's process has to be running, and unchanged requests don't start another pi. Before sending or switching models, an idle pi with stale configuration is replaced. Active work and open dialogs are kept. Explicitly choosing a model during compaction, with no open dialog, stops that process and resumes the saved session with the new choice. This stop happens outside the chat's command lock, since pi may compact before acknowledging a prompt. A prompt not accepted by pi stays available to retry. It's one list for all projects. A provider that a project-local extension adds isn't listed, but `set_model` still takes it. New chats use the last available model/reasoning pair. If its model is unavailable, they take pi's default pair from the same catalog snapshot. Missing default reasoning becomes Medium, or Off on a model without reasoning. Changing an existing chat's picks also remembers them for the next new chat. Each model's thinking levels are worked out from its model object, the way pi's `getSupportedThinkingLevels` does: only `off` without `reasoning`, and `xhigh` and `max` only when its `thinkingLevelMap` names them.
- **Project picker:** a new chat's folder chip opens an anchored popover in both Mac windows, with search, folder rows and a checkmark, followed by fixed New project and Don't work in a project actions. New project uses Rune's native folder picker. Choosing a folder adds it only to the draft until the first message creates its pi session; there is no separate project registry. A late project-list response keeps a folder chosen while it loaded.
- **Composer:** the model, thinking level and context use come from the chat's process (`get_state`, `get_session_stats`) while it runs. Otherwise they come from the index, which keeps the latest model and level from the file, and the context size from the last answer's usage, as pi estimates it. The context window then comes from the model list. Both apps ask again at most every 8 seconds while the chat works, since each step grows the context, and once more when the run ends. Asking never starts pi. Clicking the context ring replaces it with two compact figures: current context tokens/capacity in thousands (such as `75k/272k`) and the whole chat's cost in US dollars (such as `$0.75`). Clicking the figures restores the ring. Both Mac windows and the phone use this toggle; it is not a tooltip or toast. After compaction, an unknown context count reads `—` until pi reports it again. Totals include assistant and tool usage, compactions, branch summaries, and usage entries, even on branches no longer in the context.
- **Skills** (the `$` menu): `get_commands`, keeping those with `source: "skill"`. It asks the chat's process if it's running. Otherwise, and for a new chat, it asks a short-lived `pi --mode rpc --no-session` in the chat's or project's folder, since project skills depend on it. The list isn't cached: the composer asks once, on the first `$`.
  - A skill's ref is pi's own `/skill:name`. The composer puts it where `$name` was typed, but pi runs a skill only from the start of a prompt, so the engine moves the first one there. Later ones stay as text.
  - pi saves a run skill as a `<skill>` block with the whole `SKILL.md`, then the request. The transcript and the list show it as `/skill:name` and the request, as pi's own view does (`parseSkillBlock`).
- **Images:** sent in the `prompt` command as base64. A pasted image is also saved as a PNG under `uploads/` in the app's state folder, and a Markdown link to it, `[Image 1](path)`, goes into the text where it was pasted. The agent then knows where each image sits in the message and has a file to work with. Removing the image's thumbnail removes its link. The files are kept, since the sent messages link to them. A file dropped or pasted from Finder is linked where it is.
- **Markdown:** you write Markdown, and pi gets it as typed. The composer and your sent messages show the source shaded: headings and bold heavier, list markers and link targets faint, link text in the accent colour. Shift+Return in a list item starts the next item, numbered one up, and in an empty item ends the list. Links in a sent message open: a path in its app, a URL in the browser. The phone renders your messages as Markdown, and shows a link to a file on the Mac as its label, since the image chips open the images.
- **Live view:** see [live view and the file](#live-view-and-the-file).
- **Stopping idle processes:** once a chat settles (`agent_settled`) and has no open question, the engine closes its process's stdin after 5 idle minutes. pi then shuts down cleanly, and the next message starts it again with `--session <file>`. A restart costs about 270 ms even for a large session, while a loaded process holds up to ~300 MB, so stopping idle processes is the biggest memory saving.
- **Crashes:** if pi exits during a run, the chat shows as interrupted, and the next message starts pi again. A pi that crashes takes its process group with it, so its tools don't keep changing files. A clean exit leaves background jobs the agent started, such as a dev server, as a terminal pi does.
- **Engine restarts:** the engine writes a small runtime file, `runtime.json`, with each pi process's pid, its session, and whether a run is in flight. On start, it stops any pi it finds there that's still running, before any new pi may write to that session. Its stdin closed with the old engine, so it gets SIGTERM, and its process group gets SIGKILL if it's still running after 10 s. The file keeps each process's start time, so a reused pid is left alone. A run that was in flight shows as interrupted. It doesn't count as finished, since the file alone can't tell: pi writes the same `aborted` reply on its way out as for a Stop. The mark is kept in the index with the chat's last activity, so it lasts across restarts and ends with the chat's next activity.
- **Shutdown:** on SIGTERM, the engine closes every pi's stdin and gives them 10 s to finish before it kills what's left, with their process groups. Runs it stops this way show as interrupted once it's back, without notices, since the stop was on purpose. No new pi starts once shutdown begins.

### Live view and the file

Two sources describe a working chat: pi's events, and the file. pi sends `message_end` before it appends the entry to the file (`agent-session.js`, pi 1.5.2), and its events carry no entry ids. So:

1. During a run, the engine keeps a live tail built from the events: messages from `message_end`, and the streaming one from its `message_update` deltas, replaced by its final block content as each block ends.
2. The transcript is the file's path to its leaf, followed by the tail's messages that the file doesn't have yet. A message and its entry share their role, their `timestamp` and, for a tool result, its `toolCallId`, which is how they're matched. A turn that's partly written shows as one message.
3. When the run settles, the file has every message, and the tail is dropped.

The tail keeps finished messages without tool output or image data, which no view shows and which can be megabytes. Streamed text grows in place.

A message's id is its first entry's id. A turn that's only in the tail has a `live-<n>` id until pi writes its first entry.

A message is a list of parts, in order: text, thinking, tool calls, images and errors. A tool call carries pi's tool name, its subject (a command, a path or a pattern), its id and its state: running, done or failed. A `!command` you ran is a `bash` call, by its entry's id. An image carries its index in its message. Their details load on request, so a page stays small: `tool_output` has a call's whole arguments, its output up to 1 MiB, its images and the tool's own `details`, like an edit's diff, and `chat_image` has an image's data. A running call's output comes from the live tail, and a finished one's from the file. Views group consecutive calls under one summary, and the phone leaves thinking out. A wrapper also carries compact child rows from pi's `nestedCalls` and codemode's `details.calls`, including model helpers. Execution events link live children by `parentToolCallId`; provisional codemode ids are not separate rows. The transcript scan keeps only that metadata, not nested output. On Mac, codemode expands into normal tool rows inside the outer activity group. The summary counts children rather than wrappers, and each child retains its own outcome if the script fails. Script arguments and combined output load only when their disclosure is opened; closing it drops the output. Individual child outputs are not persisted by pi.

While a chat works, `chat_view` also has its `run`: the messages pi holds for the agent (`steering`, given after its current step, and `followUp`), the automatic retry it waits on with the time the attempt starts, and why it's compacting the context (`threshold`, `overflow` or `manual`). They come from pi's `queue_update`, `auto_retry_start` and `compaction_start` events. An abort drops the queue without a `queue_update`, so the queue empties when the run settles. Compaction before `agent_start` also counts as working. A failed `compaction_end` retains its error, shows it on Mac and phone, and pauses queued sends rather than treating the run as successful.

The phone sizes a picked image down to 1600 pixels on its longest side, as JPEG, unless it's already a small image of a type models take. Models scale bigger images down anyway. The relay takes phone calls of up to 12 MiB, under the 16 MiB websocket frames the engine reads. An image sent alone gets no title from it: the next message with text gives one.

`chat_view` returns the latest 40 messages. Once you load earlier ones, it returns every message from the first one shown (`from`) to the end, with `earlier` more before it. The anchor keeps loaded messages in place as new ones come, which a count from the end can't, so there's no limit on how far back you can go. If the anchor is gone, as when the session moved to another branch, it's the latest 40 again.

Every change bumps the chat's revision. A chat's subscriber gets a feed of frames: the latest page first, with `reset`, and then, after each change, the page's message ids in order with only the messages that are new or changed. The chat, its run, the question it asks and its run changes come whole in each frame, since they're small. Once the page's first message is in the file, the feed runs from it, so new messages don't push out ones the window has; if it's gone, as after `/tree`, the next frame resets. Frames come at most every 150 ms and changes in between merge, so a client that falls behind gets the latest state, not a backlog, and a reconnect starts over with a reset. While a reply streams, a frame carries just that message: about 7 frames a second of up to 5 KB, at under 1% of a core for the release engine. The window lands those frames without springs, so it draws only when one arrives. The view is cheap to read again, since each open file is scanned once, then only for what pi appended, and only the lines of the page asked for are parsed.

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

Ludolph adds no question tool of its own. The questions it shows are extensions' dialogs: every `extension_ui_request` dialog (`select`, `confirm`, `input`, `editor`) waits for an answer, so a run would hang without one. A view gets it as pi asks it: its `id`, its `dialog`, its `title`, a `confirm`'s `message`, a `select`'s `options` and an `editor`'s `prefill`. A `select` offers its options, a `confirm` offers Yes and No, and an `input` or `editor` takes typed text, an `editor`'s on several lines. The answer is a `value`, which for a `select` must be one of its options, or a `confirm`'s `confirmed`. It goes back as an `extension_ui_response` (`docs/rpc-extension-ui.md`).

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
2. As soon as Ludolph sends a message to a chat without a title, the engine asks the sidecar model for one, while the run goes on. The model sees the chat's first prompt, or the message itself for a new chat, clipped to 2,000 characters. A new chat has its title about 3 seconds after you send. Messages an extension command handles (like `/name`) don't ask.
3. It runs `pi -p` the way zeron-tools' `pi.rs` does: in a temporary folder, with no session, tools, extensions, skills, prompt templates, context files, or MCP, with a 60-second limit, and with at most two running at once.
4. When the title comes back, the engine asks the chat's pi for its state. If the session got a name in the meantime, from you or from pi, the new title is dropped. Otherwise it stores the title with `set_session_name`. pi keeps the name until it writes the session file, then adds it as a `session_info` entry, so pi's own `/resume` shows the same title. The list shows the title at once, even before the file exists.
5. If any step fails, or the chat's pi stopped before the title came back, the next message tries again.

Titles and dictation cleanup use the **sidecar model** from Settings, or pi's default model if none is picked. It requests Low thinking, which pi clamps to the model's supported levels. Some reasoning models reject Off, even for a short title. The setting is just a model, and a small, fast one keeps the delay short. The engine only accepts a model that pi lists, and a new pick applies from the next call.

Renaming a chat by hand (`rename_chat`) goes through the same `set_session_name`, and starts the chat's pi if it has stopped, since pi owns the file. The list shows the new title at once. The engine never replaces a title that a person or pi set. Calls that change one chat (title, rename, model) go through one queue per chat, so they can't overtake each other.

A session written without a name, by pi in a terminal or by another tool, gets a title the next time it runs in Ludolph. Until then, the list shows its first prompt.

## The chat list and deleting

Ludolph's index keeps every summary in memory, so a long list costs nothing to produce. Keeping it short is the list's job:

- Each project, and **No project**, shows its 5 newest chats, each with how long ago it was last active, and its total. **Show more** adds 5. The windows remember what's expanded in memory only, so a restart shows 5 again.
- `list_chat_groups` returns every group with its newest chats, and takes a higher limit for each expanded group. A window asks for the whole list again on each list notice, so there are no offsets that drift when chats change order.
- The phone keeps its flat `list_chats`, the 200 newest chats.

"Activity" means a new user or assistant message. A new title doesn't count, so titling doesn't move a chat up the list.

A chat you're done with is deleted, not archived. Right-clicking a chat in either window's list opens a native menu: open it in the other window, **Rename…** and **Delete…**. The chat's own menu has the last two as well, and so does the phone's. Delete asks first, since it can't be undone:

- `delete_chat` stops the chat's pi if it runs, without marking the run as interrupted, then removes every copy of its session file and its saved turn changes.
- It refuses a chat another pi has open, since that pi would write the file again.
- The windows leave a deleted chat's page, and the list's notice drops its row.

## Chat details

The Mac shows a details card beside an existing chat in the main window and above its transcript in the quick window. The header button beside the quick-window and More actions shows or hides it. It starts shown, and the preference is shared by both windows and saved locally. There is no Sources section.

- **Changes** opens the current checkout's uncommitted diff, independent of agent turns. It includes staged, unstaged and untracked files across the whole repository, respecting `.gitignore`. Like turn snapshots, it uses a private index and object store, so reading it leaves the real index and repository objects untouched. A repository without commits compares against an empty tree. Existing snapshot limits apply, and a large patch is marked truncated.
- **Pull requests** lists explicit HTTPS GitHub-style PR URLs mentioned in final assistant answers on the current branch. Markdown and bare links are normalized and deduplicated. Tool output, intermediate commentary and code examples do not add PRs. The links are derived incrementally from pi's file, across the whole history rather than just the visible transcript page. There is no PR status polling, auth requirement, or second history store. Bare references such as `#123` are not resolved.
- **Usage** shows all recorded compactions, cumulative tokens in millions, and cost in US dollars. Counts include earlier branches, as pi's terminal totals do. Failed compaction attempts without a saved entry do not count.

`chat_details` returns session metadata, saved usage and current change counts. `working_changes` loads a fresh patch for the existing diff viewer, headed **Uncommitted changes** rather than **Turn changes**. A Git error leaves usage and PR links available. The card refreshes at most every eight seconds while visible, and after a run settles or its project changes. Hidden cards and windows do not poll. Reading details never starts pi or contacts the network.

## Turn changes

A completed turn's diff button sits at the right of its final reply's timestamp and copy row. A trailing compaction stays a separate note and doesn't take the reply's button.

The app shows what each run changed, as zeron-tools does now. A run is everything from the prompt that starts it until pi settles, including any steering messages sent in between. pi's own `turn_start` is one assistant response, which is too small a unit, and the engine can't pause pi to take a snapshot in the middle of a run.

The engine takes its snapshots with git, without touching your repo:

1. **Before** it forwards a prompt to an idle chat, it snapshots the working tree, and only then sends the prompt. So no tool can change a file before the snapshot.
2. When the run settles, it snapshots again. A new prompt waits for that snapshot.
3. It diffs the two snapshots and saves the patch and its line counts as `turns/<session id>/<key>.json`. The key is the timestamp of the user message that started the run, which pi reports as the run starts. `chat_view` matches it to that message's `createdAt`, and a run lasts until the next run starts, including a run still in progress, so steering messages stay in it and a completed turn's diff stays below its own reply. The feed invalidates its run-change cache when a run boundary changes or a diff finishes saving, even if the transcript ids and chat status stay the same.

A snapshot copies the repo's index to a temporary file and runs `git add -A` and `git write-tree` against that copy (`GIT_INDEX_FILE`). New file contents go to an object store of the run's own (`GIT_OBJECT_DIRECTORY`), with the repo's objects as an alternate (`GIT_ALTERNATE_OBJECT_DIRECTORIES`). So the repo gets no new objects, your index and staged changes stay as they are, and `.gitignore` is respected, which keeps ignored secrets out. Once the patch is saved, the run's store is deleted, so nothing needs pruning. A store left by a stopped engine is deleted at start. A warm snapshot takes about 30 ms on zeron-tools and on Ludolph's own repo.

Limits:

- A snapshot that takes more than 10 s, or a changed file over 10 MB, skips that run's changes, with a line in the engine's log.
- Binary files show as changed, without content. A patch over 1 MB is cut at a line's end and marked `truncated`.
- The saved patches have a 50 MB budget, and the oldest go first.
- A run whose pi exits before it settles has no changes.

Folders that aren't git repos get no run changes. Two chats working in the same repo at once see each other's changes in their diffs.

## Engine API

The app and the relay link use the same tool layer, as `connect` and the Companion do now.

- **App ↔ engine:** a Unix socket, `~/Library/Application Support/Ludolph/engine.sock`, with mode 0600. It carries JSON lines: calls with an `id` and their replies, plus subscriptions that push what changes, each push naming its subscription by the request's `id`, so a client can follow one chat twice: a notice when the chat list changes, which the client follows with `list_chats`, and a chat's feed of frames (see above). `chat_view` stays for one-off reads and for loading earlier messages, and can answer "unchanged" for the version the client already shows. Pushing instead of polling means no 1-second timer while a window is hidden. Notices that wait merge into one, so a slow client gets no backlog. A request line can be up to 64 MiB, which leaves room for a message's screenshots.
- **Relay ↔ engine:** the relay protocol (`relay-proto`). Calls are sealed (see [Trust](#trust)). Only the phone's tools are allowed, and they're checked in the engine, not on the relay. The relay address must be `https://`, or `http://` for a relay on this Mac, so the token is never sent in the clear.

### Tools

The app and the phone call the same tools:

| Tool | In Ludolph |
|---|---|
| `list_chats` | From the index |
| `list_chat_groups` | From the index, by project |
| `list_projects` | The distinct cwds in the index |
| `chat_view` | The [transcript](#live-view-and-the-file)'s latest messages, a page at a time |
| `tool_output` | A tool call's arguments and output |
| `chat_image` | An image in a message |
| `send_message` | `prompt`, or queue/steer while working, with up to 8 PNG, JPEG, GIF or WebP images |
| `edit_queued_message`, `delete_queued_message` | Change one unsent draft by id, or delete it |
| `steer_queued_message` | Send one queued draft now, steering the current run without changing the queue default |
| `resume_queue` | Send the oldest waiting prompt while idle |
| `create_chat` | Start pi in the project's folder or a scratch folder, set the picked model, then `prompt` |
| `interrupt_chat` | `clear_queue` and `abort`. A send that hasn't reached pi yet is cancelled. |
| `respond_to_input` | Answers the [open question](#questions-from-the-agent) with that id, as an `extension_ui_response` |
| `rename_chat` | `set_session_name` (see [titles](#titles)) |
| `move_chat` | Stop an idle pi, move its session to the destination project, and update its header's cwd |
| `delete_chat` | Stop pi, then remove the session's files |
| `turn_changes` | Saved turn changes |
| `composer_info` | Branch (`git`), then the model, thinking level and context use (see [pi processes](#pi-processes)) |
| `set_chat_model` | `set_model`, `set_thinking_level` |
| `list_models` | Cached `get_available_models` |
| `list_skills` | `get_commands` (see [pi processes](#pi-processes)) |
| `polish_dictation` | The sidecar model through `pi -p` |
| `take_over_chat` | Turns sending back on for a chat [another pi](#sessions-open-elsewhere) has open |

The socket also has `get_settings` and `set_settings`. The relay passes `get_settings` on, so the phone shows tool calls as the Mac does, and refuses `set_settings`: a relay mustn't change settings.

In the phone app, a Mac whose engine isn't running is offline. A chat open in a terminal pi shows a **Take over** button, which asks first, since two pis writing one file split the chat. Until a chat has a title, it shows its first prompt. The relay protocol is at version 5. Version 2 dropped the `Engine` message, since a linked Mac is a running engine, version 3 changed messages' parts, version 4 dropped harnesses and took pi's dialog shape, and version 5 added watches.

An open chat streams to the phone rather than being polled. The phone opens `GET /api/watch/{machine}`, a server-sent event stream, with the chat in a sealed request (see [Trust](#trust)), and the relay asks the Mac to `watch` it. The Mac pushes the chat's feed (see [Live view and the file](#live-view-and-the-file)) as `frame`s, at most one per 200 ms, since each goes over the network. Each frame becomes one event. When the phone leaves, the relay sends `unwatch`, and a frame for a watch nobody holds gets another. A phone that falls 32 frames behind loses its stream rather than a frame, since a skipped frame would leave its page wrong. The stream also ends when the Mac's link does. The phone connects again, and the first frame resets its page. The phone closes the stream while it's hidden and drops one that sent nothing for 40 s; the relay sends a keep-alive every 15 s. Earlier messages still come from `chat_view`. The service worker leaves `/api/` alone, since a stream never ends.

## App

The views are shared by both windows. They're today's Companion views, with the Companion's theme, syntax colors, animations, and activity rows, not Zeron's:

- the chat view: transcript, tool-call rows, questions, copy buttons, links
- the turn changes page
- the composer: images, dictation, skills, model, stop
- the chat list rows and status filters

They live in a `ui` crate. Each window gives them its own layout.

- **Main window:** a normal, resizable window. The sidebar has **New chat** (⌘N), search (⌘F), and the projects as folders with their chats under them, each with a **+** for a new chat in it, collapsible, and with **Show more**. Search shows a field in place of its row while it's in use, and Escape clears it. The sidebar and toolbar are a shade lighter than the chat. The window opens where it was last, if that's still on a display. ⌃⌘S hides the sidebar. The open chat is on the right, its transcript capped at a reading width. Its toolbar has the title and project, an action to open the chat in the quick window, and the chat's menu. The quick window's header can open its chat here too, and hides as it does. The window shows a Dock icon while open, and the app can be pinned to the Dock. Closing it keeps the draft but drops the open chat and the window's GPU surface, so the app goes back near its hidden size. **Settings** (⌘, from either window, or the sidebar's last row) replaces the chat on the right, and Escape goes back to it.
- **Chat actions:** right-clicking a chat in either list and the open chat's More menu offer **Copy Chat ID** (pi's session id), rename and delete. Move a chat only by dragging its row onto another project's heading in the chat panel. The destination highlights during the drag, including collapsed projects. Dropping elsewhere or pressing Escape cancels. Both windows follow a moved chat into its new project.
- **Quick window:** today's Companion: floating and slightly transparent, shown and hidden with the global shortcut (⌃⌥Space until you record another in Settings). It follows the chat list and the open chat through the socket's feeds while it shows, and drops them when it hides. Its 1-second tick runs only while something on screen counts: a toast, the working clock, a copy check, a retry countdown, or, on the settings page, a phone link that's connecting. The list loads the 200 latest chats and shows 10 per project, with **Show more**.
- **Dictation:** Apple's speech recognizer, as now, then cleaned up by the sidecar model.
- **Accessibility:** the composer is a multiline text field named **Message**, with a stable `composer` identifier on Mac. Rune exposes its text and selection through AccessKit, and applies accessibility edits through the same editor as typing. Native spelling suggestions are enabled. Fettle's selected-text shortcut, correction paste and undo work. Automatic underlines do not work at the pinned Rune revision: text runs omit the direction AccessKit needs to return word bounds, so range rectangles are empty. The pinned AccessKit macOS adapter does not implement a setter for selected text alone, so tools that require only that setter cannot apply corrections.

### Settings

The engine keeps most of them, since it's the one using them and it runs while the app is closed. The app reads and saves them with `get_settings` and `set_settings`, which take all the settings at once. The phone can read them but not change them. A change shows at once, and goes back with a toast if the engine refuses it.

- Sidecar model, for titles and dictation (`sidecarModel`), picked from pi's models with a search, or pi's default model, which the picker names (`list_models` asks a fresh pi's `get_state` for it)
- Notifications on the phone only while you're away from the Mac, or always (`notify`: `away` or `always`)
- Open at login (`openAtLogin`, off until you turn it on)
- Send while working (`sendBehavior`: `queue`, the default, or `steer`), shared by Mac and phone
- Phone: link a phone with a QR code, see whether the link is connected, or unlink (`link_phone`, `stop_linking_phone`, `get_phone_link`, `unlink_phone`; see "Phone and relay")
- Tool calls collapsed or expanded (`expandTools`): the engine's, so both windows and the phone draw them alike. A reply's work between two texts, its tool calls and thinking, is one group on a rail. The setting only decides whether the group under way shows open while the agent works; a group folds once the agent writes again, and a click opens or closes one group either way. Once a turn ends, everything before its last text folds under **Worked for 6m 56s** over a rule, whatever the setting.
- Global shortcut: the app's own, since it registers it, so it's in `ui.json`. The recorder registers the new chord before it lets the old one go, so a chord another app holds is refused and the old one stays. If the saved one is taken when the app starts, the page says so.

## Phone and relay

The relay, its Kubernetes manifests, and the phone app live in `crates/relay`, with the app in `assets`. The phone app keeps its look and flows.

- **Linking a phone:** only from the Mac. **Link a phone** in Settings has the engine (`link_phone`) open a WebSocket on the relay's `/pair`, with the Mac's token if it has one. The relay answers with a single-use code that works for 10 minutes, kept in memory only. The engine also makes a 32-byte secret for this phone. Settings shows `https://<relay>/#/pair/<code>/<secret>` as a QR code. Both are in the fragment, so no server log keeps them, and the phone never sends the secret on (see [Trust](#trust)). The phone's camera opens that page in the phone app, which claims the code:
  - A signed-in phone claims it at once.
  - Otherwise the phone signs in with its passkey, or creates one. It may create one through a Mac the relay already knows, or on a relay with no passkey yet, which makes the first link set the relay up. A Mac the relay doesn't know needs a signed-in phone.
  - A passkey, not just a cookie, because a home-screen web app doesn't share Safari's cookies.
  - A Mac new to the relay gets its token over the waiting socket, saves it in the Keychain, and connects at once. `relay.json` keeps only the relay's address.
  `get_phone_link` says whether the link is connected and how a pairing stands, `stop_linking_phone` drops the code, and `unlink_phone` disconnects, forgets the link, and has the relay forget the Mac (`Up::Forget`). Settings asks before it unlinks. The phone can't call these. The relay's address defaults to the maintainer's, and Settings takes another before the first link.
- **Calls that must run once:** the relay gives up on a call after 30 s, but a send can take longer, say while pi starts or a big repo is snapshotted. So the phone tags `send_message`, `create_chat` and `respond_to_input` with an op id. The engine runs an op once, as its own task, and keeps its result for 10 minutes (256 per phone). If it's still running after 25 s, the engine answers `pending` instead of cancelling it. On `pending`, a timeout, or a dropped connection, the phone asks again with the same id for up to 3 minutes, so it never shows "Not sent" for a message that arrived, or sends it twice.
- **Battery:** the phone shows the Mac's battery. The engine reads it from IOKit when it connects, and again on each `com.apple.system.powersources` notification, about once a minute while charging or on battery. It sends it only when it changed, so nothing polls.
- **Protocol 7:** the relay refuses other versions. The maintainer's relay runs at ludolph.iyazerski.dev, deployed by `.github/workflows/deploy-relay.yml`.
- **Notifications:** finished, needs you, failed, and interrupted. As now, a notice is dropped if the Mac's keyboard and mouse were used in the last 2 minutes, unless set to always notify. It isn't sent later. The engine raises these from pi's events: a run you stopped stays quiet, and so does a question answered before its notice went out. Runs by another pi aren't watched, so they don't notify.
- **Self-hosting:** `crates/relay/README.md` explains how to run the relay (container image and manifests). Whoever links the first phone sets up the relay, so link one right after it starts.

### Trust

What the phone and a Mac say is sealed end to end, so the relay routes it without reading or forging it. An agent can run commands, so a relay that could forge `send_message` could run code on your Mac.

- **Keys:** each linked phone has its own 32-byte secret, made by the engine and passed in the QR code's fragment. Two HMAC-SHA256s of it give the phone's id and an AES-256-GCM key. The engine keeps the secrets in the Keychain with its relay token. The phone keeps the key in IndexedDB as a WebCrypto key it can't export, and the secret goes nowhere. The relay sees only the id, which it routes by. The engine is `crates/engine/src/seal.rs`, the phone app is `crates/relay/assets/seal.js`, and they share test vectors.
- **What's sealed:** each call with its tool and arguments, each answer, each `watch` request and every frame, and each notice. A sealed message is `version || nonce || ciphertext || tag` in base64url, and its text starts with the time it was sealed.
- **Binding:** the associated data names what a message is. An answer is bound to its call's nonce, and a frame to its watch's nonce and its place in the stream. So the relay can't swap answers, reorder, drop, or replay frames, or pass one kind off as another without the open failing. The engine refuses a call or watch sealed more than 5 minutes ago, or one it already took.
- **Notifications:** the engine seals a notice once for each phone. The relay keeps, with each browser's push subscription, the ids of the phones it holds keys for, and pushes the matching copy. The service worker opens it. One it can't open still shows, since a push must, as a plain "A chat changed".
- **A phone without the key:** a browser signed in with the passkey but never linked to a Mac shows that Mac as not linked, and asks you to link it from the Mac's Settings.
- **What the relay still sees:** which Macs a phone uses, when, how much, and who signs in. Whoever controls it can also block or delay messages.
- **What it doesn't cover:** the relay serves the phone app's code, so a hostile relay could serve code that sends the key out. Run it yourself, or trust whoever does. Unlinking forgets every phone's secret. Removing a Mac in the phone app forgets its key there.

Passkey sign-in, origin checks, and revoking a Mac stay. The Mac's relay token and secrets are in the Keychain, which ties them to the app's signature (`make install` signs every build with one identity).

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

Measured on the quick window's release build, with a long chat: 63 MB hidden, 87 MB shown with the list, 100 MB with a chat open, all at 0% CPU while nothing changes. While a reply streams, the app takes about 4% of a core. The hidden app is 3 MB over budget.

Measured on the main window's release build (physical footprint): 10 MB started with no window, 69 MB with the main window on a new chat, 150 MB with a long chat open, and 88 MB after closing it, all at 0 to 0.3% CPU. The rest after closing is Rune's per-window state (text atlas, motion, layout), which it keeps for a window that may reopen.

Why streaming frames skip springs: Rune springs a page to its new layout, so with each frame animated, a growing reply kept the window drawing at the display's 120 Hz for the whole reply. That took 20 to 25% of a core, and fewer frames didn't change it. Frames that only grow a streaming reply now land at once, which leaves Rune drawing about 7 frames a second. Other changes still animate.

How we check:

- `ps -o rss,pcpu` sampled over a minute in each state, on every release.
- Index build time and memory with the 270 sessions on this Mac, cold and warm.
- A soak test: 5 chats working at once for 10 minutes, one of them streaming a long answer, with a phone that stops reading. The engine's memory shouldn't keep growing.

## Repo layout

A Cargo workspace, following fettle where it applies (bundle, signing, release):

```
crates/
  engine/       pi processes, index, titles, deleting, turn changes, tools
  ui/           shared Rune views
  relay/        the relay server and the phone app
  relay-proto/  the Mac ↔ relay messages
  json-file/    atomic JSON files
src/main.rs     the `ludolph` binary: the app, `engine`, and `uninstall` for the launch agents
assets/         the app's Info.plist template, icon (drawn by crates/relay/assets/make_icons.py) and DMG background
scripts/        bundle.sh, dmg.py, and the local signing identity
install.sh      installs or updates the latest release
```

## Releases

As in fettle, each push to `main` that changes the app publishes `vX.Y.n` (`.github/workflows/release.yml`): X.Y from `Cargo.toml`, and n counting the earlier X.Y releases. It ships only what passes CI. The release has `Ludolph.dmg` for people, `Ludolph.zip` for `install.sh` and the Homebrew cask in `iyazerski/homebrew-tap`, and a checksum for each. CI sets `LUDOLPH_VERSION`, which the engine, the windows and `Info.plist` show.

The app is signed with a self-signed certificate, the same for every release (`MACOS_CERT_P12`), so macOS keeps the microphone, speech and Keychain grants across updates. `install.sh` and the release workflow check that certificate's hash. It isn't notarized, so macOS asks once before the first start.

Settings checks for a newer release each time it opens, and offers to update. The app downloads `Ludolph.zip` with the system's curl, checks its checksum and certificate, swaps the app in one rename, and starts again. The engine's launchd job names the version, so the new app's changed job restarts the engine too, which stops running chats; Settings says so beside the button.

## Existing sessions

There's nothing to import. The first time the engine starts, it indexes every session under `~/.pi/agent/sessions`, whichever tool ran pi, and lists each by its `cwd`, title and activity.

- **Copies:** a session file copied under another folder keeps its session id. The index lists a session once, by the copy written last.
- **Untitled sessions:** see [titles](#titles).

## Milestones

1. **Engine:** the index, pi processes, status, titles, deleting, turn changes, the socket API, and the relay link. The phone works against it.
2. **Quick window:** the Companion ported to the engine's socket, with the views moved into `ui`.
3. **Main window:** the sidebar, Dock icon, and app bundle, signed like fettle.
4. **Settings:** including linking a phone.
5. The first public release.

## Alternatives considered

- **One process for the engine and the app.** It would save a few MB, but quitting or updating the app would stop every agent and cut off the phone.
- **Embedding pi through its TypeScript SDK.** It would put a JavaScript runtime in the engine and tie it to pi's internals. RPC is pi's documented interface for other languages, and it keeps each chat in its own process.
- **Ludolph's own transcript store, as Zeron has.** It would be a second copy of every chat, and it would drift from pi's files. pi's files already have everything the views need.
- **Polling instead of FSEvents and pushed updates.** Polling costs CPU while nothing happens, which is what the budget rules out.
- **Grouping projects by git root.** It gave the same groups as the cwd on this Mac's sessions, for a git call per folder.
