# Agent notes

Ludolph is a macOS app for [pi](https://pi.dev) coding agents, written in Rust with the [Rune](https://github.com/anomalyco/rune) UI framework.

- It supports pi only, through pi's RPC mode. Don't add support for other agents.
- pi's session files are the only store of chat history. Keep Ludolph's own state to a small index.
- Keep CPU and memory use low: measure before and after changes on hot paths.
- Rune is pre-release: keep it pinned to one tested revision.
- `docs/` holds only what the code and merged PRs don't show: architecture and its invariants, pi behavior Ludolph relies on, the relay's trust model, and performance budgets. Read a doc only when your change touches its area.
- Update a doc only when an invariant, a pi assumption, the trust model, or a budget changes. Don't describe features, UI, or history there.
