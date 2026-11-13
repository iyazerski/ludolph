# Contributing

Thanks for helping with Ludolph. Bug reports, ideas and pull requests are all welcome.

## Report a bug or ask for a feature

[Open an issue](https://github.com/iyazerski/ludolph/issues/new/choose) and pick a template. For questions and ideas, use [Discussions](https://github.com/iyazerski/ludolph/discussions). For security problems, follow [SECURITY.md](SECURITY.md) instead.

## Make a change

You need macOS on Apple silicon and [Rust](https://rustup.rs). Ludolph is early, and [docs/design.md](docs/design.md) describes the plan.

```sh
make test
make engine    # run a development engine, with its own files in target/dev
make app       # run the windows on it, beside your installed Ludolph
make install   # build Ludolph.app and put it in /Applications
```

The development engine and app don't touch the installed ones, but the installed app keeps the ⌃⌥Space shortcut. `make install` signs with a local identity it makes on first use, so macOS keeps the microphone grant across installs.

Before you open a pull request:

1. Install the hooks with [prek](https://github.com/j178/prek): `prek install`. They run `cargo fmt`, `cargo check` and `cargo clippy`.
2. Run `make test`.
3. If you changed dependencies, run `make licenses` and commit `THIRD_PARTY_LICENSES.md`. It needs [cargo-about](https://github.com/EmbarkStudios/cargo-about).
4. Keep pull requests small and focused on one change.

## Code style

- Follow the code next to your change.
- Give every item a doc comment, and every `unsafe` block a `// SAFETY:` comment.
- Use comments to say why, not what.

`main` must always work.

By contributing, you agree that your work is released under the [MIT License](LICENSE). Please follow the [Code of Conduct](CODE_OF_CONDUCT.md).
