# pi behavior Ludolph relies on

Ludolph uses pi's RPC mode (`docs/rpc.md`, `docs/rpc-extension-ui.md`), its session files (`docs/session-format.md`), and the package exports its extensions may import. These are the behaviors the code depends on that pi's docs don't promise, or that are easy to miss. Check them again when you upgrade pi. They were last checked with pi 1.5.2.

## RPC

- Records are JSON lines, split only on `\n`. Responses match requests by `id`. pi's stdout must be read all the time.
- `pi --mode rpc --session-id <uuid>` doesn't write the session file until the first prompt.
- `message_end` arrives before pi appends the entry to the file (`agent-session.js`). Events carry no entry ids.
- An abort drops pi's queue without a `queue_update`.
- pi writes the same `aborted` reply on exit as for a Stop. So `runtime.json` alone can't tell whether an interrupted run finished.
- An `extension_ui_request` dialog (`select`, `confirm`, `input`, `editor`) blocks until it gets an answer. When its `timeout` runs out, pi resolves it without telling the client. An extension command can wait on a dialog outside a run, so a pi with an open question isn't idle.
- `set_session_name` is kept in memory until pi writes the file, then saved as a `session_info` entry.
- `get_available_models` lists one set of models for all projects. A provider added by a project-local extension isn't listed, but `set_model` still accepts it.
- Supported thinking levels come from the model object, as pi's `getSupportedThinkingLevels` works them out: only `off` without `reasoning`, and `xhigh` and `max` only when `thinkingLevelMap` names them.
- pi may compact before it acknowledges a prompt.
- RPC mode refuses to open a session whose `cwd` no longer exists.
- A fresh `pi --mode rpc` answers `get_state` in about 250 ms. Resuming a 45 MB session takes about 270 ms. A loaded pi uses 135 to 300 MB.

## Extensions

- `--extension <file>` loads a plain `.js` file next to installed packages. Its imports of `@earendil-works/pi-coding-agent`, `@earendil-works/pi-ai` and `typebox` resolve to pi's own copies, so the bundles leave them out.
- `pi config` lists what `DefaultPackageManager.resolve()` returns, plus the built-in extensions, which `resolve()` lists only when given their names. Its switches write `+path` or `-path` to the user's `extensions` and `skills`, or to a package's filters. `resources.js` does the same.
- An extension command sent as an RPC `prompt` runs before pi answers, so its `notify` comes first.
- `ctx.modelRegistry` lists providers only through their models, so a provider whose catalog is empty until sign-in (`typesafe` in 1.5.2) is missing. `ModelRuntime.create()`, which `pi auth` uses, lists every provider, so the account list and sign-in use it. `provider.auth` tells account sign-in (`oauth`) from API key login (`apiKey.login`).
- An RPC pi reads its input while a command runs, so a long command can wait on `ctx.ui.input` and `ctx.ui.select` dialogs.
- `ctx.ui.setStatus` reaches the client as a fire-and-forget `extension_ui_request` with `statusKey` and `statusText`. The quota extension reports through it, so its readings arrive whenever they finish, not with any event.

## Subscription quota

The Codex and Antigravity sources aren't documented, so either may change without notice. Gateways have no fixed provider id, so the extension finds them by name in the provider id or the base URL's host.

- `openai-codex`: `GET {baseUrl}/wham/usage`, the endpoint behind Codex CLI's `/status`, with the access token and the `chatgpt-account-id` from its claims. `rate_limit` has the 5h and weekly windows, or is `null` on an enterprise plan. `spend_control.individual_limit` has a workspace budget in dollars.
- `antigravity-acp`: the `antigravity-acp:quota:request` event of `pi-antigravity-acp-provider` (0.1.19 or later). It parses agy's `/usage`, which spends no quota. Gemini models share a "Gemini Models" group, and some also have their own, such as "Gemini 4 Argon".
- Bifrost: `GET /api/governance/virtual-keys/quota` with the virtual key. It's the one governance route a virtual key may call. Its `budgets` have no reset time, so the extension adds `reset_duration` to `last_reset`.
- LiteLLM: `GET /key/info` with the key, then `GET /user/info?user_id=` for its user, at the proxy's root. A budget may be on either. A key limited to `llm_api_routes` gets 403 from both.

## Sign-in

- `ModelRuntime.login(provider, type, interaction)` reports its steps as typed events (`auth_url`, `device_code`, `info`, `progress`), and asks typed prompts (`text`, `secret`, `select`, `manual_code`). It never opens the browser itself; `pi auth login` does that, so Ludolph does too.
- An account sign-in starts a local callback server for the browser, and also asks a `manual_code` prompt in case the page can't reach it. The prompt's signal aborts when the callback wins, and an RPC dialog then resolves with no message to the client.
- An account sign-in's method `select` lists the browser first, and `pi auth login` takes it without asking.
- `logout` removes only what `auth.json` stores. Environment variables and models.json stay.

## Session files

- The first line is the header, with `id` and `cwd`.
- The latest `session_info` entry's `name` is the title.
- Entries form a tree (`id`, `parentId`). `/tree` leaves old branches in the file, so the transcript is the path from the last entry appended.
- `/fork` and `/clone` copy entries, with their ids, into a new file.
- pi loads a session once and then only appends (`session-manager.js`). It may still rewrite a whole file, for example when it migrates an old version on load.
- pi doesn't lock session files or keep them open between writes (checked with `lsof`). Two pis on one session fork its history.
- The `timestamp` of a user message in the file matches the one pi reports in its events. Custom messages differ: the event has role `custom`, but the file entry has type `custom_message` and a new timestamp from when pi saves it. Match these by `customType` and content, not time.

## Wishlist

- An advisory lock (`flock`) on session files, so Ludolph can tell when a terminal pi has a chat open.
- A move RPC, so moving a chat doesn't need Ludolph to rewrite the header.
