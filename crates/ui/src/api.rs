//! The bridge from a window to the engine: calls and subscriptions run on
//! a tokio runtime and hand their results back to the UI thread through
//! the window's handle. The windows share one connection and one prefs
//! writer ([`Shared`]).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use rune::{AppHandle, Tx};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::State;
use crate::client::{Client, Subscription};
use crate::prefs::Prefs;

/// How often the UI checks what's due while it ticks: the working clock,
/// a toast's end.
const TICK: Duration = Duration::from_secs(1);

/// What the windows share: the runtime, the engine connection and the
/// prefs writer.
#[derive(Clone)]
pub struct Shared {
    rt: Handle,
    client: Client,
    prefs: Arc<watch::Sender<Value>>,
    uploads: Arc<Path>,
}

impl Shared {
    /// Connect to the engine at `socket` and write the prefs to
    /// `prefs_path`, both on `rt`. Pasted images are saved under `uploads`.
    pub fn start(rt: Handle, socket: PathBuf, prefs_path: PathBuf, uploads: PathBuf) -> Self {
        let client = {
            let _entered = rt.enter();
            Client::start(socket)
        };
        let prefs = Arc::new(write_prefs(&rt, prefs_path));
        Self {
            rt,
            client,
            prefs,
            uploads: uploads.into(),
        }
    }
}

pub struct Api {
    rt: Handle,
    client: Client,
    app: AppHandle<State>,
    /// The other window, for handing it a chat and redrawing it after the
    /// prefs it shows change.
    peer: AppHandle<State>,
    prefs: Arc<watch::Sender<Value>>,
    ticking: watch::Sender<bool>,
    uploads: Arc<Path>,
}

impl Api {
    /// Serve the window behind `app` over `shared`'s connection: tell it
    /// when the engine comes and goes, and tick it while it asks.
    pub fn start(shared: &Shared, app: AppHandle<State>, peer: AppHandle<State>) -> Self {
        let Shared {
            rt,
            client,
            prefs,
            uploads,
        } = shared.clone();
        let mut up = client.up();
        let to_ui = app.clone();
        rt.spawn(async move {
            loop {
                let now = *up.borrow_and_update();
                to_ui.update(move |s, _| s.engine_changed(now));
                if up.changed().await.is_err() {
                    break;
                }
            }
        });
        let (ticking, mut wanted) = watch::channel(false);
        let ticks = app.clone();
        // Nothing ticks while nothing on screen moves, as with the window hidden.
        rt.spawn(async move {
            loop {
                if wanted.wait_for(|on| *on).await.is_err() {
                    break;
                }
                let mut every = tokio::time::interval(TICK);
                every.tick().await;
                while *wanted.borrow() {
                    tokio::select! {
                        _ = every.tick() => ticks.update_if(State::tick),
                        changed = wanted.changed() => if changed.is_err() { return },
                    }
                }
            }
        });
        Self {
            rt,
            client,
            app,
            peer,
            prefs,
            ticking,
            uploads,
        }
    }

    /// Where pasted images are saved, so a message can link to them.
    pub fn uploads(&self) -> &Path {
        &self.uploads
    }

    /// Tick each second, or stop.
    pub fn tick(&self, on: bool) {
        self.ticking
            .send_if_modified(|was| std::mem::replace(was, on) != on);
    }

    /// Save the prefs off the UI thread, and redraw the other window, which
    /// shows them too. Losing them only resets what the list shows as new
    /// and the last picks.
    pub fn save_prefs(&self, prefs: &Prefs) {
        match serde_json::to_value(prefs) {
            Ok(prefs) => {
                self.prefs.send_replace(prefs);
            }
            Err(e) => eprintln!("{e:#}"),
        }
        self.peer.update(|_, _| {});
    }

    pub fn app(&self) -> &AppHandle<State> {
        &self.app
    }

    pub fn peer(&self) -> &AppHandle<State> {
        &self.peer
    }

    /// Call `tool` with `args`, then `done` with its parsed answer (or why
    /// it failed) on the UI thread.
    pub fn call<T: DeserializeOwned + Send + 'static>(
        &self,
        tool: &'static str,
        args: Value,
        done: impl FnOnce(&mut State, Result<T, String>, &mut Tx) + Send + 'static,
    ) {
        self.run(
            move |client| async move {
                let answer = client.call(tool, args).await?;
                serde_json::from_value(answer).map_err(|e| format!("unexpected {tool} answer: {e}"))
            },
            done,
        );
    }

    /// Run `work` on the runtime, then `done` with its result on the UI
    /// thread: for work that isn't one call.
    pub fn run<R: Send + 'static, F: Future<Output = R> + Send + 'static>(
        &self,
        work: impl FnOnce(Client) -> F,
        done: impl FnOnce(&mut State, R, &mut Tx) + Send + 'static,
    ) {
        let app = self.app.clone();
        let work = work(self.client.clone());
        self.rt.spawn(async move {
            let result = work.await;
            app.update(move |s, tx| done(s, result, tx));
        });
    }

    /// Follow `topic` with `args`, with `on` getting each push on the UI
    /// thread, until the handle drops.
    pub fn subscribe(
        &self,
        topic: &'static str,
        args: Value,
        on: impl Fn(&mut State, Value, &mut Tx) + Send + Sync + 'static,
    ) -> Subscription {
        let (app, on) = (self.app.clone(), Arc::new(on));
        self.client.subscribe(
            topic,
            args,
            Arc::new(move |record| {
                let on = on.clone();
                app.update(move |s, tx| on(s, record, tx));
            }),
        )
    }
}

/// Write the newest prefs sent to `path`, one write at a time: saves made
/// during a slow write collapse into one.
fn write_prefs(rt: &Handle, path: PathBuf) -> watch::Sender<Value> {
    let (saves, mut latest) = watch::channel(Value::Null);
    rt.spawn(async move {
        while latest.changed().await.is_ok() {
            let prefs = latest.borrow_and_update().clone();
            let path = path.clone();
            let written =
                tokio::task::spawn_blocking(move || ludolph_json_file::save(&path, &prefs));
            if let Ok(Err(e)) = written.await {
                eprintln!("{e:#}");
            }
        }
    });
    saves
}
