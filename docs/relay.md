# Phone and relay

The phone app talks to the Mac through a relay the user hosts (`crates/relay`, setup in its README). An agent can run commands, so a relay that could forge `send_message` could run code on the Mac. Everything here follows from that: the relay routes messages but must not be able to read or forge them.

## Trust

- **Keys:** the engine makes a 32-byte secret for each linked phone and passes it in the QR code's URL fragment, so no server log sees it. Two HMAC-SHA256s of the secret give the phone's id and an AES-256-GCM key. The relay sees only the id. The engine keeps secrets in the Keychain on macOS, and elsewhere in a file only the user can read (mode 0600) in the state folder, so any process running as the user can read them there. The phone keeps a non-exportable WebCrypto key in IndexedDB, and the secret goes nowhere.
- **Sealed:** every call with its tool and arguments, every answer, watch request, frame, and notice. `crates/engine/src/seal.rs` and `crates/relay/assets/seal.js` share the format and test vectors.
- **Binding:** the associated data names what a message is. An answer is bound to its call's nonce, and a frame to its watch and its place in the stream. So the relay can't swap, reorder, drop, or replay messages without opening failing.
- **Replays:** the engine refuses calls sealed more than 5 minutes ago, or ones it already took. It saves accepted nonces in `relay-replays/<phone id>.json` before running a request, so a restart doesn't reopen the window. If that file can't be read or saved, requests are blocked, not let through.
- **Engine-side checks:** only the phone's tools are allowed, and the engine enforces that. The relay passes `get_settings` but refuses `set_settings`. The phone never gets tool output.
- **Relay address:** `https://`, or `http://` only for a relay on the same Mac, so the token never goes in the clear.

What the relay still sees: which Macs a phone uses, when, and how much. It can block or delay messages. It also serves the phone app's code, so a hostile relay could serve code that leaks the key. Run it yourself, or trust whoever does.

## Linking

Phones are linked only from the Mac. The relay gives a single-use code, valid for 10 minutes and kept only in memory. A phone signs in with a passkey, since a home-screen web app doesn't share Safari's cookies. The first link on a relay with no passkey sets the relay up, so link a phone right after it starts. Removing the last phone also has the relay forget the Mac.

## Calls that must run once

The relay gives up on a call after 30 s, but a send can take longer while pi starts or a big repo is snapshotted. So `send_message`, `create_chat`, and `respond_to_input` carry an op id. The engine runs each op once and keeps its result for 10 minutes. After 25 s it answers `pending` instead of cancelling. The phone retries with the same id for up to 3 minutes. Once an attempt may have reached the Mac, a later offline answer doesn't end the retries.

## Streaming and notifications

An open chat streams to the phone over server-sent events (`GET /api/watch/{machine}`), as the Mac's feed frames. A phone 32 frames behind loses its stream rather than a frame, since a skipped frame would leave its page wrong. Reconnecting starts with a reset. The service worker leaves `/api/` alone, since a stream never ends.

Notices are sealed once per phone. A notice the service worker can't open still shows, as "A chat changed", because a web push must show something. Notices are dropped, not delayed, if the Mac was used in the last 2 minutes, unless the user picked "always". Off macOS the engine can't tell, so it always sends them.

The relay protocol version is `PROTOCOL` in `crates/relay-proto`. The relay refuses other versions.
