//! Ludolph's windows: the main window, with the chats in a sidebar beside
//! the open chat, and the quick window, a small floating one shown and
//! hidden on a global shortcut. Both talk to the engine over its socket.

mod api;
mod app;
mod client;
mod data;
mod dictation;
mod labels;
mod message;
mod prefs;
mod view;

use std::cell::{Cell, OnceCell, RefCell};
use std::fs::File;
use std::path::PathBuf;
use std::rc::Rc;

use anyhow::{Context, Result, bail};
use rune::{App, AppSpec, Chrome, MacWindowOptions, RunOptions, WindowOptions, WindowSpec};

use self::api::{Api, Shared};
use self::app::{State, Window};
use self::prefs::{Frame, Prefs};
use self::view::{CORNER, on_key, view};

/// NSWindow collection behavior: on every Space, and over full-screen apps.
const ALL_SPACES: usize = 1 << 0;
const FULL_SCREEN_AUXILIARY: usize = 1 << 8;
/// NSFloatingWindowLevel: above normal windows, under menus.
const FLOATING: isize = 3;
const SIZE: (f32, f32) = (420.0, 620.0);
const MIN_SIZE: (f32, f32) = (340.0, 420.0);
const MAIN_SIZE: (f32, f32) = (1120.0, 760.0);
const MAIN_MIN_SIZE: (f32, f32) = (720.0, 480.0);

/// The first spot: the primary display's top right, under the menu bar. It's an
/// initial position, not a placement, so later opens keep where the user dragged it.
fn top_right(displays: &[rune::Display]) -> Option<(i32, i32)> {
    let display = displays.iter().find(|d| d.primary).or(displays.first())?;
    let px = |points: f32| (f64::from(points) * display.scale).round() as i32;
    let right = display.position.0 + i32::try_from(display.size.0).ok()?;
    Some((right - px(SIZE.0 + 24.0), display.position.1 + px(56.0)))
}

/// Where the main window was last, if that's still on a display.
fn saved_frame(prefs: &Prefs, displays: &[rune::Display]) -> Option<Frame> {
    let frame = prefs.main_frame?;
    let (x, y) = frame.position;
    let shown = displays.iter().any(|d| {
        let (w, h) = (i64::from(d.size.0), i64::from(d.size.1));
        let (left, top) = (i64::from(d.position.0), i64::from(d.position.1));
        (left..left + w).contains(&i64::from(x)) && (top..top + h).contains(&i64::from(y))
    });
    shown.then_some(frame)
}

/// The main window's first spot: the middle of the primary display.
fn centered(displays: &[rune::Display]) -> Option<(i32, i32)> {
    let display = displays.iter().find(|d| d.primary).or(displays.first())?;
    let px = |points: f32| (f64::from(points) * display.scale).round() as i32;
    let (w, h) = (
        i32::try_from(display.size.0).ok()?,
        i32::try_from(display.size.1).ok()?,
    );
    Some((
        display.position.0 + (w - px(MAIN_SIZE.0)) / 2,
        display.position.1 + (h - px(MAIN_SIZE.1)) / 2,
    ))
}

pub struct Opts {
    /// The engine's folder, with its socket; the app keeps its own state there too.
    pub state_dir: PathBuf,
    /// Start with no window, as at login, rather than with the main window.
    pub hidden: bool,
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
    let prefs = Rc::new(RefCell::new(Prefs::load(&prefs_path)));
    let shared = Shared::start(rt.handle().clone(), socket, prefs_path);
    let hotkey = Rc::new(Cell::new(None));
    let expand_tools = Rc::new(Cell::new(false));
    let main_window = Rc::new(OnceCell::new());
    // The writer runs on the runtime, which may not get to the last save as the app quits.
    let last_prefs = prefs.clone();

    let spec = |size, min_size| AppSpec {
        min_size: Some(min_size),
        fonts: rune_theme_opencode::FONTS,
        families: rune_theme_opencode::FAMILIES,
        on_key: Some(on_key),
        setup: Some(|s, waker| s.images = Some(rune::ImageCache::new(waker))),
        ..AppSpec::new(
            "Ludolph",
            size,
            || unreachable!("init() below builds the state"),
            view,
        )
    };
    // Both start windowless, so a window is made only once it first opens.
    let quick = App::new(spec(SIZE, MIN_SIZE)).windowless();
    let main = App::new(spec(MAIN_SIZE, MAIN_MIN_SIZE)).windowless();
    let (quick_handle, main_handle) = (quick.handle(), main.handle());
    // Registered once the app runs, so the settings can change it.
    quick_handle.update(|s, _| s.register_saved_shortcut());
    if !opts.hidden {
        main_handle.update(|s, _| s.open());
    }
    let quick = {
        let (shared, prefs, hotkey) = (shared.clone(), prefs.clone(), hotkey.clone());
        let expand_tools = expand_tools.clone();
        let (me, peer) = (quick_handle, main_handle.clone());
        quick
            .init(move || {
                let api = Api::start(&shared, me, peer);
                State::new(
                    api,
                    Window::Quick,
                    prefs,
                    hotkey,
                    expand_tools,
                    Rc::default(),
                )
            })
            .on_visibility(|s, shown, _| s.visibility_changed(shown))
    };
    let main = {
        let (peer, handle, prefs) = (quick.handle(), main_window.clone(), prefs.clone());
        main.init(move || {
            let api = Api::start(&shared, main_handle, peer);
            State::new(api, Window::Main, prefs, hotkey, expand_tools, handle)
        })
        .chrome(Chrome {
            full_size_content: true,
            // Centred in the toolbar band, as `SplitView` asks.
            traffic_lights: Some((16.0, 17.0)),
            drag_regions: true,
        })
        .on_visibility(|s, shown, _| {
            // Only once it's on screen: an app that becomes regular while its window
            // orders in gets the window back at a stray, tiny frame.
            if shown {
                s.api.app().set_dock_icon(true);
            }
            s.visibility_changed(shown)
        })
        .on_close(State::closed)
        .on_quit(|s, _| s.keep_frame())
        // The Dock icon, or opening the app again from Finder or Spotlight.
        .on_reopen(|s, _, _| {
            s.open();
            true
        })
    };
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
        let mut saved = saved_frame(&prefs.borrow(), displays);
        let main_options = WindowOptions {
            resizable: true,
            visible: false,
            position: saved.map(|f| f.position).or_else(|| centered(displays)),
            physical_size: saved.map(|f| f.size),
            min_size: Some(MAIN_MIN_SIZE),
            ..Default::default()
        };
        vec![
            WindowSpec::from_app("quick", quick).configured_options(options),
            WindowSpec::from_app("main", main)
                .options(main_options)
                .on_ready(move |handle| {
                    // AppKit centres a window first shown during launch, wherever it was made.
                    if let Some(frame) = saved.take()
                        && handle
                            .placement()
                            .is_some_and(|at| at.position != frame.position)
                    {
                        handle.set_position(frame.position);
                    }
                    // The same handle each time the window opens again.
                    drop(main_window.set(handle.clone()));
                }),
        ]
    });
    let prefs_path = opts.state_dir.join("ui.json");
    if let Err(e) = ludolph_json_file::save(&prefs_path, &*last_prefs.borrow()) {
        eprintln!("{e:#}");
    }
    drop(rt);
    drop(lock);
    Ok(())
}
