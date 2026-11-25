mod api;
mod app;
mod client;
mod data;
mod dictation;
mod labels;
mod markup;
mod paths;
mod prefs;
mod update;
mod view;
#[cfg(windows)]
mod wsl;

use std::cell::{Cell, OnceCell, RefCell};
#[cfg(not(windows))]
use std::fs::File;
use std::path::PathBuf;
use std::rc::Rc;

#[cfg(not(windows))]
use anyhow::bail;
use anyhow::{Context, Result};
use rune::{
    App, AppSpec, Chrome, ColorScheme, MacWindowOptions, RunOptions, WindowOptions, WindowSpec,
};

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

fn bottom_right(displays: &[rune::Display]) -> Option<(i32, i32)> {
    let display = displays.iter().find(|d| d.primary).or(displays.first())?;
    let px = |points: f32| (f64::from(points) * display.scale).round() as i32;
    let right = display.position.0 + i32::try_from(display.size.0).ok()?;
    let bottom = display.position.1 + i32::try_from(display.size.1).ok()?;
    Some((right - px(SIZE.0), bottom - px(SIZE.1)))
}

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

pub const VERSION: &str = match option_env!("LUDOLPH_VERSION") {
    Some(version) => version,
    None => env!("CARGO_PKG_VERSION"),
};

pub struct Opts {
    pub state_dir: PathBuf,
    pub hidden: bool,
}

pub fn run(opts: Opts) -> Result<()> {
    std::fs::create_dir_all(&opts.state_dir)
        .with_context(|| format!("creating {}", opts.state_dir.display()))?;
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let folder = opts
            .state_dir
            .canonicalize()
            .context("resolving the state folder")?;
        let bytes: Vec<u8> = folder
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect();
        let hash = ring::digest::digest(&ring::digest::SHA256, &bytes);
        let suffix: String = hash
            .as_ref()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        // Separate state folders must keep separate instances, including development builds.
        if !rune::single_instance(&format!("dev.iyazerski.ludolph.{suffix}")) {
            return Ok(());
        }
    }
    // macOS forwards launches through LaunchServices; Windows forwards them above.
    #[cfg(not(windows))]
    let lock = {
        let path = opts.state_dir.join("app.lock");
        let lock = File::create(&path).with_context(|| format!("opening {}", path.display()))?;
        if lock.try_lock().is_err() {
            bail!("Ludolph is already running");
        }
        lock
    };
    #[cfg(windows)]
    rune::set_app_icon_resource(1);
    #[cfg(target_os = "windows")]
    if let Err(error) = rune_notify::register(&rune_notify::Identity::new(
        "dev.iyazerski.ludolph",
        "Ludolph",
    )) {
        eprintln!("registering notifications: {error}");
    }
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("starting the runtime")?;
    let link = client::Link::new(&opts.state_dir).context("finding the engine")?;
    let prefs_path = opts.state_dir.join("ui.json");
    let (mut prefs, existed) = Prefs::load(&prefs_path);
    let previous_version = prefs.last_version.clone();
    let updated_notice = prefs
        .note_launch_version(VERSION, existed)
        .then(|| VERSION.to_owned());
    if prefs.last_version != previous_version
        && let Err(e) = ludolph_json_file::save(&prefs_path, &prefs)
    {
        eprintln!("{e:#}");
    }
    let prefs = Rc::new(RefCell::new(prefs));
    let shared = Shared::start(rt.handle().clone(), link, prefs_path);
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
    let quick = App::new(spec(SIZE, MIN_SIZE))
        .windowless()
        .color_scheme(ColorScheme::Dark)
        // Keeps it out of screenshots and screen sharing, so it can stay open during a call.
        .exclude_from_capture(true);
    let main = App::new(spec(MAIN_SIZE, MAIN_MIN_SIZE))
        .windowless()
        .color_scheme(ColorScheme::Dark);
    let (quick_handle, main_handle) = (quick.handle(), main.handle());
    // Ludolph has only a dark look; setting it app-wide also themes native surfaces, menus,
    // dialogs, and Windows/Linux chrome under a light system.
    main_handle.set_color_scheme(ColorScheme::Dark);
    let _job_notices = shared.job_notices(&main_handle);
    let dock = main_handle.clone();
    quick_handle.update(|s, _| s.register_saved_shortcut());
    if !opts.hidden {
        main_handle.update(|s, _| s.open());
    }
    let quick = {
        let (shared, prefs, hotkey) = (shared.clone(), prefs.clone(), hotkey.clone());
        let expand_tools = expand_tools.clone();
        let updated_notice = updated_notice.clone();
        let (me, peer) = (quick_handle, main_handle.clone());
        quick
            .init(move || {
                let api = Api::start(&shared, me, peer);
                let mut state = State::new(
                    api,
                    Window::Quick,
                    prefs,
                    hotkey,
                    expand_tools,
                    Rc::default(),
                );
                state.updated_notice = updated_notice;
                state
            })
            .on_visibility(|s, shown, _| s.visibility_changed(shown))
    };
    let main = {
        let (peer, handle, prefs) = (quick.handle(), main_window.clone(), prefs.clone());
        main.init(move || {
            let api = Api::start(&shared, main_handle, peer);
            let mut state = State::new(api, Window::Main, prefs, hotkey, expand_tools, handle);
            state.updated_notice = updated_notice;
            state
        })
        .chrome(Chrome {
            full_size_content: true,
            traffic_lights: Some((16.0, 17.0)),
            drag_regions: true,
        })
        .on_visibility(|s, shown, _| {
            // Each later open; `on_ready` below covers the first, which this misses.
            if shown {
                s.api.app().set_dock_icon(true);
            }
            s.visibility_changed(shown)
        })
        .on_close(State::closed)
        .on_quit(|s, _| s.keep_frame())
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
            position: bottom_right(displays),
            min_size: Some(MIN_SIZE),
            ..Default::default()
        }
        .macos(
            // Keep a normal floating window (not a nonactivating panel) so clicking the quick
            // composer activates and accepts keyboard input cleanly.
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
                    // Not before its first frame: an app that becomes regular while its
                    // window orders in gets the window back at a stray, tiny frame.
                    dock.set_dock_icon(true);
                    // AppKit centres a window first shown during launch, wherever it was made.
                    if let Some(frame) = saved.take()
                        && handle
                            .placement()
                            .is_some_and(|at| at.position != frame.position)
                    {
                        handle.set_position(frame.position);
                    }
                    drop(main_window.set(handle.clone()));
                }),
        ]
    });
    let prefs_path = opts.state_dir.join("ui.json");
    if let Err(e) = ludolph_json_file::save(&prefs_path, &*last_prefs.borrow()) {
        eprintln!("{e:#}");
    }
    drop(rt);
    #[cfg(not(windows))]
    drop(lock);
    #[cfg(windows)]
    rune::release_instance();
    Ok(())
}
