# Performance

Ludolph should stay light. Measure before and after any change to a hot path.

## Budget

On an Apple silicon Mac, with no chat working:

| Process | Memory | CPU |
|---|---|---|
| Engine | under 30 MB | about 0%: event-driven, no polling timers |
| App, windows hidden | under 60 MB | about 0%: no ticks while hidden |
| App, main window open | under 150 MB | about 0% when nothing changes on screen |
| pi | one process per chat that's working or was used in the last 5 minutes | — |

## What keeps it there

- The engine parses session files incrementally and streams them line by line.
- Transcripts load a page at a time. Tool output and images load only when shown.
- Subscriber queues are bounded and merge pending changes, so slow clients get the latest state, not a backlog.
- Windows subscribe only while visible, and hidden windows don't poll. A 1-second tick runs only while something on screen counts down.
- Closing the main window drops its GPU surface.

## Rune lessons

- **No springs on streaming frames.** Rune springs a page to its new layout. With every frame animated, a growing reply kept the window drawing at 120 Hz for the whole reply, which took 20 to 25% of a core. Frames that only grow a reply now land at once, so Rune draws about 7 frames a second.
- **Entrances are one-off.** Nothing loops, and streaming text never animates. An entrance that mounts out of sight lands in the same commit that scrolls to it, so entrances mount hidden and rise after their first layout.
- **Key every page.** Without keys, Rune matches unkeyed parts by position and springs them from the last page's layout.
- Rune keeps per-window state (text atlas, motion, layout) after a window closes, so the app doesn't drop all the way back to its hidden size.

## How to check

- Sample `ps -o rss,pcpu` over a minute in each state of the table, on a release build.
- Time the index build, cold and warm, on a real set of sessions.
- Soak test: 5 chats working at once for 10 minutes, one streaming a long answer, with a phone that stops reading. Engine memory shouldn't keep growing.
