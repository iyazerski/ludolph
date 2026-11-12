# Agent notes

Ludolph is a macOS app for [pi](https://pi.dev) coding agents, written in Rust with the [Rune](https://github.com/anomalyco/rune) UI framework.

- It supports pi only, through pi's RPC mode. Don't add support for other agents.
- pi's session files are the only store of chat history. Keep Ludolph's own state to a small index.
- Keep CPU and memory use low: measure before and after changes on hot paths.
- Rune is pre-release: keep it pinned to one tested revision.
- `docs/design.md` describes the design. Update it when a change departs from it.
