<div align="center">

# Ludolph

**A fast, native Mac app for pi coding agents.**

Ludolph runs [pi](https://pi.dev) agents on your Mac, groups their chats by project, and lets you follow and steer them from a full window, a floating window on a global shortcut, or your phone.

[![macOS](https://img.shields.io/badge/macOS-Apple%20silicon-black?logo=apple)](#install)
[![Release](https://img.shields.io/github/v/release/iyazerski/ludolph)](https://github.com/iyazerski/ludolph/releases/latest)
[![License: MIT](https://img.shields.io/badge/license-MIT-green)](LICENSE)
[![Built with Rust](https://img.shields.io/badge/built%20with-Rust-orange?logo=rust)](https://www.rust-lang.org)
[![Buy me a coffee](https://img.shields.io/badge/buy%20me%20a%20coffee-FFDD00?logo=buymeacoffee&logoColor=black)](https://buymeacoffee.com/iyazerski)

</div>

## Install

Ludolph needs a Mac with Apple silicon and [pi](https://pi.dev), signed in to a model provider.

- **Download:** get [Ludolph.dmg](https://github.com/iyazerski/ludolph/releases/latest/download/Ludolph.dmg), open it, and drag Ludolph into Applications.
- **Homebrew:** `brew install --cask iyazerski/tap/ludolph`
- **Terminal:** `curl -fsSL https://raw.githubusercontent.com/iyazerski/ludolph/main/install.sh | sh`

Ludolph isn't notarized by Apple, so macOS blocks it the first time you open it. Go to System Settings > Privacy & Security and click **Open Anyway**. Every release is signed with the same certificate, and `install.sh` checks it.

To build it from source instead, run `make install`. It builds Ludolph.app, puts it in /Applications, and opens it.

## Use

- **Chats:** the main window lists your pi sessions by project, whichever tool started them, and you can start, follow and steer chats there. Right-click a chat to rename it, delete it, or open it in the other window.
- **Quick window:** ⌃⌥Space shows a floating window over any app. You can record another shortcut in Settings (⌘,).
- **Phone:** choose **Link a phone** in Settings and scan the QR code with your phone's camera. It opens the phone app, where you sign in with a passkey. You can link several phones, and one phone can follow several Macs. The phone app goes through a relay, by default the maintainer's at ludolph.iyazerski.dev, and what the phone and your Mac say is end-to-end encrypted, so the relay can't read it. You can host your own: see [crates/relay](crates/relay/README.md).
- **In the background:** the app starts its engine, which keeps running when you quit the app, so agents and the phone carry on until you log out. Nothing starts at login unless you turn on **Open at login** in Settings. The engine logs to `~/Library/Logs/Ludolph/engine.log`.
- **Uninstall:** run `/Applications/Ludolph.app/Contents/MacOS/ludolph uninstall` to stop the engine and remove the login item, then delete the app. Ludolph's own files are in `~/Library/Application Support/Ludolph`. Your chats are pi's, so they stay.

Ideas and questions are welcome in [Discussions](https://github.com/iyazerski/ludolph/discussions).

## Why Ludolph

- **Made for pi only.** It drives pi through pi's RPC mode, with no extra agent engine in between.
- **Light on your Mac.** A native Rust app built on [Rune](https://github.com/anomalyco/rune), with no Electron and no web view. Idle agents don't keep a process running.
- **pi's sessions are the history.** Ludolph reads pi's own session files and keeps only a small index beside them, so it never holds a second copy of your chats.
- **Tidy by default.** It titles new chats with a small model as soon as you send the first message. Each project shows its five newest chats, and **Show more** lists the rest.
- **Three ways to follow your agents.** A full window with chats grouped by project, a floating window on a global shortcut, and a phone web app with notifications.
- **Your phone, privately.** The phone connects through a small relay, with passkey sign-in, and everything it says to your Mac is end-to-end encrypted. Your Mac dials out, so no VPN or open ports are needed, and you can host the relay yourself.

## The name

π is also called Ludolph's number, after Ludolph van Ceulen, who spent years working out its first 35 digits by hand.

## License

[MIT](LICENSE). By contributing, you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).
