//! Ludolph's windows. For now that's the quick window: a small floating
//! window with the chats, shown and hidden on a global shortcut. It talks to
//! the engine over its socket.

mod api;
mod app;
mod client;
mod data;
mod dictation;
mod labels;
mod message;
mod prefs;
mod view;

use std::fs::File;
use std::path::PathBuf;

use anyhow::{Context, Result, bail};
use rune::{App, AppSpec, MacWindowOptions, RunOptions, WindowOptions, WindowSpec};

use self::api::Api;
use self::app::State;
use self::prefs::Prefs;
use self::view::{CORNER, on_key, view};

/// NSWindow collection behavior: on every Space, and over full-screen apps.
const ALL_SPACES: usize = 1 << 0;
const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
/// NSFloatingWindowLevel: above normal windows, under menus.
const FLOATING: isize = 3;
const SIZE: (f32, f32) = (420.0, 620.0);
const MIN_SIZE: (f32, f32) = (340.0, 420.0);

/// The shortcut that shows and hides the quick window, until it's a setting.
pub const SHORTCUT: &str = "ctrl+alt+space";

/// The first spot: the primary display's top right, under the menu bar. It's an
/// initial position, not a placement, so later opens keep where the user dragged it.
fn top_right(displays: &[rune::Display]) -> Option<(i32, i32)> {
    let display = displays.iter().find(|d| d.primary).or(displays.first())?;
    let px = |points: f32| (f64::from(points) * display.scale).round() as i32;
    let right = display.position.0 + i32::try_from(display.size.0).ok()?;
    Some((right - px(SIZE.0 + 24.0), display.position.1 + px(56.0)))
}

pub struct Opts {
    /// The engine's folder, with its socket; the app keeps its own state there too.
    pub state_dir: PathBuf,
    /// Show the window at once, rather than on the shortcut.
    pub show: bool,
}

/// Run the app until it quits. It owns the main thread, as macOS needs;
/// the engine's calls run on a small tokio runtime beside it.
///
/// # Errors
///
/// Fails if another copy of the app is running, or the runtime can't start.
pub fn run(opts: Opts) -> Result<()> {
    std::fs::create_dir_all(&opts.state_dir)
        .with_context(|| format!("creating {}", opts.state_dir.display()))?;
    // Two would both take the shortcut, and each press would toggle twice.
    let lock_path = opts.state_dir.join("app.lock");
    let lock =
        File::create(&lock_path).with_context(|| format!("opening {}", lock_path.display()))?;
    if lock.try_lock().is_err() {
        bail!("Ludolph is already running");
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("starting the runtime")?;
    let socket = opts.state_dir.join("engine.sock");
    let prefs_path = opts.state_dir.join("ui.json");
    let prefs = Prefs::load(&prefs_path);
    let handle = rt.handle().clone();

    let spec = AppSpec {
        min_size: Some(MIN_SIZE),
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        on_key: Some(on_key),
        setup: Some(|s, waker| s.images = Some(rune::ImageCache::new(waker))),
        ..AppSpec::new(
            "Ludolph",
            SIZE,
            || unreachable!("init() below builds the state"),
            view,
        )
    };
    let app = App::new(spec).windowless();
    let me = app.handle();
    if opts.show {
        me.update(|s, _| s.toggle_window());
    }
    let app = app
        .init(move || State::new(Api::start(handle, socket, me, prefs_path), prefs))
        .global_shortcut(SHORTCUT, |s, _| s.toggle_window())
        .on_visibility(|s, shown, _| s.visibility_changed(shown));
    rune::run_windows_with_args(RunOptions { accessory: true }, [], move |displays| {
        let options = WindowOptions {
            transparent: true,
            borderless: true,
            always_on_top: true,
            resizable: true,
            visible: false,
            position: top_right(displays),
            min_size: Some(MIN_SIZE),
            ..Default::default()
        }
        .macos(
            // Not a nonactivating panel: Rune moves a panel's content into an NSPanel that
            // `.window_drag()` regions can't move.
            MacWindowOptions::default()
                .level(FLOATING)
                .collection_behavior(ALL_SPACES | FULL_SCREEN_AUXILIARY)
                .corner_radius(CORNER),
        );
        vec![WindowSpec::from_app("quick", app).configured_options(options)]
    });
    drop(rt);
    drop(lock);
    Ok(())
}
