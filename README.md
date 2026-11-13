<div align="center">

# Ludolph

**A fast, native Mac app for pi coding agents.**

Ludolph runs [pi](https://pi.dev) agents on your Mac, groups their chats by project, and lets you follow and steer them from a full window, a floating window on a global shortcut, or your phone.

[![macOS](https://img.shields.io/badge/macOS-Apple%20silicon-black?logo=apple)](#status)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org)
[![Buy me a coffee](https://img.shields.io/badge/buy%20me%20a%20coffee-FFDD00?logo=buymeacoffee&logoColor=black)](https://buymeacoffee.com/iyazerski)

</div>

## Status

Ludolph is early. There are no releases yet. To build it from source:

```sh
make install   # builds Ludolph.app, puts it in /Applications, and opens it
/Applications/Ludolph.app/Contents/MacOS/ludolph link <code>   # links it to your phone
```

The app starts its engine in the background, and it keeps running when you quit the app, until you log out. Nothing starts at login unless you turn on **Open at login**, which comes with Settings. ⌃⌥Space shows the quick window. The link code comes from the phone app's **Add a Mac**, on a relay you host: see [crates/relay](crates/relay/README.md). `ludolph uninstall` stops the engine and removes the app from login. The engine logs to `~/Library/Logs/Ludolph/engine.log`.

Ideas and questions are welcome in [Discussions](https://github.com/iyazerski/ludolph/discussions).

## Why Ludolph

- **Made for pi only.** It drives pi through pi's RPC mode, with no extra agent engine in between.
- **Light on your Mac.** A native Rust app built on [Rune](https://github.com/anomalyco/rune), with no Electron and no web view. Idle agents don't keep a process running.
- **pi's sessions are the history.** Ludolph reads pi's own session files and keeps only a small index beside them, so it never holds a second copy of your chats.
- **Tidy by default.** It titles new chats with a small model and archives all but the most recent chats of each project. Archived chats stay searchable and can be resumed.
- **Three ways to follow your agents.** A full window with chats grouped by project, a floating window on a global shortcut, and a phone web app with notifications.
- **Your phone, your relay.** The phone connects through a small relay that you host, with passkey sign-in. Your Mac dials out, so no VPN or open ports are needed.

## The name

π is also called Ludolph's number, after Ludolph van Ceulen, who spent years working out its first 35 digits by hand.

## License

[MIT](LICENSE). By contributing, you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).
