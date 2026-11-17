use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use rune::{AppHandle, Tx};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio::runtime::Handle;
use tokio::sync::watch;

use crate::State;
use crate::client::{Client, Link, Subscription};
use crate::prefs::Prefs;

const TICK: Duration = Duration::from_secs(1);

#[derive(Clone)]
pub struct Shared {
    rt: Handle,
    client: Client,
    prefs: Arc<watch::Sender<Value>>,
}

impl Shared {
    pub fn start(rt: Handle, link: Link, prefs_path: PathBuf) -> Self {
        let client = {
            let _entered = rt.enter();
            Client::start(link)
        };
        let prefs = Arc::new(write_prefs(&rt, prefs_path));
        Self { rt, client, prefs }
    }
}

pub struct Api {
    rt: Handle,
    client: Client,
    app: AppHandle<State>,
    peer: AppHandle<State>,
    prefs: Arc<watch::Sender<Value>>,
    ticking: watch::Sender<bool>,
}

impl Api {
    pub fn start(shared: &Shared, app: AppHandle<State>, peer: AppHandle<State>) -> Self {
        let Shared { rt, client, prefs } = shared.clone();
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
        }
    }

    pub fn tick(&self, on: bool) {
        self.ticking
            .send_if_modified(|was| std::mem::replace(was, on) != on);
    }

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

/// Collapse pending prefs writes to the latest value while one save is in progress.
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
