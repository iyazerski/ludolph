//! ludolph-relay: a tiny relay between a phone browser and the Ludolph
//! engines on your Macs.
//!
//! Each Mac dials out to `/connect` ([`hub`]), the phone signs in with a
//! passkey ([`auth`]), and its calls are routed to the chosen Mac. A phone
//! links by scanning a Mac's QR code ([`pair`]). What the phone and a Mac
//! say is sealed with a key the relay never sees, so it routes it unread.
//!
//! It serves plain HTTP behind a TLS-terminating proxy. API calls must be
//! JSON POSTs from the app's own origin, with the session cookie
//! (`SameSite=Strict`), so other sites can't make them.

mod auth;
mod hub;
mod pair;
mod push;
mod store;

use std::convert::Infallible;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use axum::extract::{DefaultBodyLimit, Path, Query, Request, State, WebSocketUpgrade};
use axum::http::{Method, StatusCode, header};
use axum::middleware::{self, Next};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use axum_extra::TypedHeader;
use axum_extra::extract::cookie::CookieJar;
use axum_extra::headers::Authorization;
use axum_extra::headers::authorization::Bearer;
use clap::Parser;
use ludolph_relay_proto::{CONNECT_PATH, PAIR_PATH, Sealed};
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;
use webauthn_rs::prelude::{PublicKeyCredential, RegisterPublicKeyCredential};

use auth::Auth;
use hub::Hub;
use pair::Pairings;
use push::{Payload, Push, Subscription};

/// The largest tool call the phone sends, which leaves room for a message's
/// photos (8 of up to about 1 MiB, in base64, then sealed in base64 again).
/// It goes to the Mac as one websocket frame, and the engine reads frames of
/// up to 24 MiB.
const MAX_CALL: usize = 20 << 20;
use store::{Machine, Store};

/// The app's files: plain HTML, CSS, and JS, no build step.
const ASSETS: &[(&str, &str, &[u8])] = &[
    (
        "/",
        "text/html; charset=utf-8",
        include_bytes!("../assets/index.html"),
    ),
    (
        "/app.css",
        "text/css; charset=utf-8",
        include_bytes!("../assets/app.css"),
    ),
    (
        "/app.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../assets/app.js"),
    ),
    (
        "/hljs.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../assets/hljs.js"),
    ),
    (
        "/file-icons.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../assets/file-icons.js"),
    ),
    (
        "/seal.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../assets/seal.js"),
    ),
    (
        "/sw.js",
        "text/javascript; charset=utf-8",
        include_bytes!("../assets/sw.js"),
    ),
    (
        "/manifest.webmanifest",
        "application/manifest+json",
        include_bytes!("../assets/manifest.webmanifest"),
    ),
    (
        "/icon.svg",
        "image/svg+xml",
        include_bytes!("../assets/icon.svg"),
    ),
    (
        "/icon-192.png",
        "image/png",
        include_bytes!("../assets/icon-192.png"),
    ),
    (
        "/icon-512.png",
        "image/png",
        include_bytes!("../assets/icon-512.png"),
    ),
    (
        "/icon-maskable.png",
        "image/png",
        include_bytes!("../assets/icon-maskable.png"),
    ),
    (
        "/badge.png",
        "image/png",
        include_bytes!("../assets/badge.png"),
    ),
];

#[derive(Parser)]
#[command(version, about)]
struct Opts {
    /// The address people open, like https://ludolph.example.com (passkeys
    /// are bound to its host)
    #[arg(long, env = "PUBLIC_URL")]
    public_url: Url,
    /// Where to listen
    #[arg(long, env = "BIND_ADDR", default_value = "0.0.0.0:8080")]
    bind: SocketAddr,
    /// Where the passkeys, Macs, and push key are kept
    #[arg(long, env = "DATA_DIR", default_value = "data")]
    data_dir: PathBuf,
}

#[derive(Clone)]
struct App {
    store: Arc<Store>,
    auth: Arc<Auth>,
    hub: Arc<Hub>,
    pairings: Arc<Pairings>,
    push: Arc<Push>,
    /// The public URL's origin, which every API call must come from.
    origin: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let opts = Opts::parse();
    let store = Arc::new(Store::open(&opts.data_dir)?);
    let app = App {
        auth: Arc::new(Auth::new(&opts.public_url)?),
        push: Arc::new(Push::open(&opts.data_dir)?),
        hub: Arc::default(),
        pairings: Arc::default(),
        origin: opts.public_url.origin().ascii_serialization(),
        store,
    };
    if !app.store.has_passkey() {
        eprintln!("no passkey yet: link a phone from a Mac's Ludolph Settings to make one");
    }
    let mut router = Router::new();
    for &(path, mime, bytes) in ASSETS {
        router = router.route(
            path,
            get(move || async move {
                // Always revalidate, so a new release shows up at once.
                (
                    [
                        (header::CONTENT_TYPE, mime),
                        (header::CACHE_CONTROL, "no-cache"),
                    ],
                    bytes,
                )
            }),
        );
    }
    let signed_in = Router::new()
        .route("/api/machines", post(machines))
        .route("/api/machines/remove", post(remove_machine))
        .route(
            "/api/call/{machine}",
            post(call_tool).layer(DefaultBodyLimit::max(MAX_CALL)),
        )
        .route("/api/watch/{machine}", get(watch_chat))
        .route("/push/key", post(push_key))
        .route("/push/subscribe", post(push_subscribe))
        .route("/push/unsubscribe", post(push_unsubscribe))
        .route("/push/test", post(push_test))
        .route("/api/pair/claim", post(claim_pairing))
        .route_layer(middleware::from_fn_with_state(app.clone(), require_session));
    let router = router
        .merge(signed_in)
        .route("/healthz", get(|| async { "ok" }))
        .route("/auth/status", post(auth_status))
        .route("/auth/register/start", post(register_start))
        .route("/auth/register/finish", post(register_finish))
        .route("/auth/login/start", post(login_start))
        .route("/auth/login/finish", post(login_finish))
        .route("/auth/logout", post(logout))
        .route("/auth/pair", post(pairing))
        .route(CONNECT_PATH, get(connect))
        .route(PAIR_PATH, get(pair))
        .layer(middleware::from_fn_with_state(app.clone(), check_origin))
        .with_state(app);
    let listener = tokio::net::TcpListener::bind(opts.bind)
        .await
        .with_context(|| format!("binding {}", opts.bind))?;
    eprintln!("listening on {} for {}", opts.bind, opts.public_url);
    axum::serve(listener, router)
        .with_graceful_shutdown(shutdown()?)
        .await?;
    Ok(())
}

/// Resolve on Ctrl-C or SIGTERM. Kubernetes stops pods with SIGTERM, and as
/// PID 1 in its container the relay would ignore it without a handler, so
/// each deploy would wait out the grace period.
fn shutdown() -> Result<impl std::future::Future<Output = ()>> {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term = signal(SignalKind::terminate()).context("listening for SIGTERM")?;
    Ok(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = term.recv() => {}
        }
        eprintln!("shutting down");
    })
}

/// Refuse POSTs from any page but the app's own.
async fn check_origin(State(app): State<App>, request: Request, next: Next) -> Response {
    if request.method() == Method::POST {
        let origin = request
            .headers()
            .get(header::ORIGIN)
            .and_then(|o| o.to_str().ok());
        // Browsers always send Origin on POST; a missing one isn't a browser.
        if origin != Some(app.origin.as_str()) {
            return failure(StatusCode::FORBIDDEN, "wrong origin");
        }
    }
    next.run(request).await
}

async fn require_session(State(app): State<App>, request: Request, next: Next) -> Response {
    if signed_in(&app, &CookieJar::from_headers(request.headers())) {
        next.run(request).await
    } else {
        failure(StatusCode::UNAUTHORIZED, "sign in first")
    }
}

fn signed_in(app: &App, jar: &CookieJar) -> bool {
    auth::session_cookie(jar).is_some_and(|t| app.store.session_valid(t))
}

async fn auth_status(State(app): State<App>, jar: CookieJar) -> Response {
    Json(json!({
        "signedIn": signed_in(&app, &jar),
        "hasPasskey": app.store.has_passkey(),
    }))
    .into_response()
}

#[derive(Deserialize)]
struct RegisterStart {
    /// The pairing code that allows it, when not signed in.
    pair: Option<String>,
}

/// Start creating a passkey: when signed in, or through a pairing code that
/// may make one (`pair`).
async fn register_start(
    State(app): State<App>,
    jar: CookieJar,
    Json(body): Json<RegisterStart>,
) -> Response {
    let code = body.pair.as_deref().and_then(|c| app.pairings.code(c));
    let allowed = code.is_some_and(|c| c.known || !app.store.has_passkey());
    if !signed_in(&app, &jar) && !allowed {
        return failure(StatusCode::FORBIDDEN, "sign in with your passkey first");
    }
    respond(app.auth.start_register(&app.store))
}

#[derive(Deserialize)]
struct PairCode {
    code: String,
}

/// What the phone's pairing page needs: the Mac, and how this phone may
/// claim its code.
async fn pairing(State(app): State<App>, jar: CookieJar, Json(body): Json<PairCode>) -> Response {
    let Some(code) = app.pairings.code(&body.code) else {
        return failure(StatusCode::NOT_FOUND, "this code expired");
    };
    Json(json!({
        "name": code.name,
        "known": code.known,
        "hasPasskey": app.store.has_passkey(),
        "signedIn": signed_in(&app, &jar),
    }))
    .into_response()
}

/// Link the code's Mac to this signed-in phone: add the Mac if it's new,
/// and tell it.
async fn claim_pairing(State(app): State<App>, Json(body): Json<PairCode>) -> Response {
    let Some(claim) = app.pairings.claim(&body.code) else {
        return failure(StatusCode::NOT_FOUND, "this code expired");
    };
    let (machine, token) = match claim.machine.clone() {
        Some(machine) => (machine, None),
        None => match app.store.add_machine(&claim.name) {
            Ok((machine, token)) => (machine, Some(token)),
            Err(e) => return failure(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}")),
        },
    };
    let new = token.is_some();
    if !claim.finish(token) && new {
        // The Mac left before it got its token, so nothing can use the record.
        drop(app.store.remove_machine(&machine.id));
        return failure(StatusCode::GONE, "the Mac stopped waiting");
    }
    Json(json!({ "machine": machine })).into_response()
}

#[derive(Deserialize)]
struct Finish<T> {
    flow: String,
    credential: T,
}

async fn register_finish(
    State(app): State<App>,
    jar: CookieJar,
    Json(body): Json<Finish<RegisterPublicKeyCredential>>,
) -> Response {
    // The flow itself proves setup was allowed when it started.
    let result = app
        .auth
        .finish_register(&app.store, &body.flow, &body.credential)
        .and_then(|()| app.store.new_session());
    signed_in_response(jar, result)
}

async fn login_start(State(app): State<App>, Json(_): Json<Value>) -> Response {
    respond(app.auth.start_login(&app.store))
}

async fn login_finish(
    State(app): State<App>,
    jar: CookieJar,
    Json(body): Json<Finish<PublicKeyCredential>>,
) -> Response {
    let result = app
        .auth
        .finish_login(&app.store, &body.flow, &body.credential)
        .and_then(|()| app.store.new_session());
    signed_in_response(jar, result)
}

fn signed_in_response(jar: CookieJar, session: Result<String>) -> Response {
    match session {
        Ok(token) => (auth::set_cookie(jar, token), Json(json!({ "ok": true }))).into_response(),
        Err(e) => failure(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

async fn logout(State(app): State<App>, jar: CookieJar) -> Response {
    if let Some(token) = auth::session_cookie(&jar)
        && let Err(e) = app.store.end_session(token)
    {
        return failure(StatusCode::INTERNAL_SERVER_ERROR, format!("{e:#}"));
    }
    (auth::clear_cookie(jar), Json(json!({ "ok": true }))).into_response()
}

async fn machines(State(app): State<App>, Json(_): Json<Value>) -> Response {
    let list: Vec<Value> = app
        .store
        .machines()
        .into_iter()
        .map(|m| {
            let mut value = json!(m);
            value["status"] = json!(app.hub.status(&m.id));
            value
        })
        .collect();
    Json(json!({ "machines": list })).into_response()
}

#[derive(Deserialize)]
struct MachineId {
    id: String,
}

async fn remove_machine(State(app): State<App>, Json(body): Json<MachineId>) -> Response {
    let result = app.store.remove_machine(&body.id);
    app.hub.drop_machine(&body.id);
    done(result)
}

/// Pass a phone's sealed call to a Mac, and its sealed reply back.
async fn call_tool(
    State(app): State<App>,
    Path(machine): Path<String>,
    Json(sealed): Json<Sealed>,
) -> Response {
    match app.hub.call(&machine, sealed).await {
        Ok(sealed) => Json(json!({ "sealed": sealed })).into_response(),
        Err(error) => failure(StatusCode::BAD_REQUEST, error),
    }
}

/// Stream a chat's feed from a Mac as server-sent events, one sealed frame
/// each, for the request the phone sealed in the query. The stream ends when
/// the Mac goes away or the phone falls behind; the phone then connects
/// again and starts over.
async fn watch_chat(
    State(app): State<App>,
    Path(machine): Path<String>,
    Query(sealed): Query<Sealed>,
) -> Response {
    match app.hub.watch(&machine, sealed).await {
        Ok(watch) => {
            let frames = futures_util::stream::unfold(watch, |mut watch| async move {
                let frame = match watch.frames.recv().await? {
                    Ok(sealed) => json!({ "sealed": sealed }),
                    Err(error) => json!({ "error": error }),
                };
                Some((
                    Ok::<_, Infallible>(Event::default().data(frame.to_string())),
                    watch,
                ))
            });
            // Comments keep proxies and phones from closing a quiet stream.
            Sse::new(frames)
                .keep_alive(KeepAlive::default())
                .into_response()
        }
        Err(error) => failure(StatusCode::BAD_REQUEST, error),
    }
}

/// A Mac's link: its engine, with `Authorization: Bearer <token>`.
async fn connect(
    State(app): State<App>,
    bearer: Option<TypedHeader<Authorization<Bearer>>>,
    ws: WebSocketUpgrade,
) -> Response {
    let token = bearer.as_ref().map(|TypedHeader(auth)| auth.token());
    let Some(machine) = token.and_then(|t| app.store.machine_for_token(t)) else {
        return failure(StatusCode::UNAUTHORIZED, "unknown Mac token");
    };
    let (hub, push, store) = (app.hub.clone(), app.push.clone(), app.store.clone());
    ws.on_upgrade(move |socket| {
        let known = {
            let store = store.clone();
            move |id: &str| store.has_machine(id)
        };
        let forget = {
            let store = store.clone();
            move |machine: &Machine| {
                if let Err(e) = store.remove_machine(&machine.id) {
                    eprintln!("{}: forgetting it: {e:#}", machine.name);
                }
            }
        };
        let notify = move |machine: &Machine, notices: Vec<Sealed>| {
            let several = store.machine_count() > 1;
            let machine = machine.clone();
            let push = push.clone();
            tokio::spawn(async move {
                push.broadcast(|sub| payload(&machine, &notices, several, sub))
                    .await
            });
        };
        hub.serve(machine, socket, known, notify, forget)
    })
}

/// A Mac waiting for a phone to link, with its token if it has one.
async fn pair(
    State(app): State<App>,
    bearer: Option<TypedHeader<Authorization<Bearer>>>,
    ws: WebSocketUpgrade,
) -> Response {
    let machine = match bearer {
        Some(TypedHeader(auth)) => match app.store.machine_for_token(auth.token()) {
            Some(machine) => Some(machine),
            None => return failure(StatusCode::UNAUTHORIZED, "unknown Mac token"),
        },
        None => None,
    };
    ws.on_upgrade(move |socket| async move { app.pairings.serve(socket, machine).await })
}

/// The notification for one browser of a Mac's notice: the copy sealed for
/// one of its phones, if it has one. With several Macs, it names the Mac.
fn payload(
    machine: &Machine,
    notices: &[Sealed],
    several: bool,
    sub: &Subscription,
) -> Option<Payload> {
    let notice = notices.iter().find(|n| sub.phones.contains(&n.phone))?;
    Some(Payload::Notice {
        machine: machine.id.clone(),
        mac: several.then(|| machine.name.clone()),
        sealed: notice.clone(),
    })
}

async fn push_key(State(app): State<App>, Json(_): Json<Value>) -> Response {
    Json(json!({ "publicKey": app.push.public_key() })).into_response()
}

async fn push_subscribe(State(app): State<App>, Json(sub): Json<Subscription>) -> Response {
    done(app.push.subscribe(sub).await)
}

#[derive(Deserialize)]
struct Endpoint {
    endpoint: String,
}

async fn push_unsubscribe(State(app): State<App>, Json(e): Json<Endpoint>) -> Response {
    done(app.push.unsubscribe(&e.endpoint).await)
}

async fn push_test(State(app): State<App>, Json(e): Json<Endpoint>) -> Response {
    let payload = Payload::Test {
        title: "Notifications work".into(),
        body: "You'll hear from your Macs when a chat finishes or needs you.".into(),
    };
    done(app.push.send_to(&e.endpoint, &payload).await)
}

fn respond<T: serde::Serialize>(result: Result<T>) -> Response {
    match result {
        Ok(value) => Json(value).into_response(),
        Err(e) => failure(StatusCode::BAD_REQUEST, format!("{e:#}")),
    }
}

fn done(result: Result<()>) -> Response {
    respond(result.map(|()| json!({ "ok": true })))
}

fn failure(status: StatusCode, error: impl Into<String>) -> Response {
    (status, Json(json!({ "error": error.into() }))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_browser_gets_the_notice_sealed_for_its_phone_and_names_the_mac_only_when_there_are_several()
     {
        let mac = Machine {
            id: "m".into(),
            name: "Work Mac".into(),
        };
        let sealed = |phone: &str| Sealed {
            phone: phone.into(),
            data: format!("for {phone}"),
        };
        let notices = [sealed("a"), sealed("b")];
        let sub = |phones: &[&str]| Subscription {
            endpoint: "https://push.example/1".into(),
            keys: push::SubscriptionKeys {
                p256dh: String::new(),
                auth: String::new(),
            },
            phones: phones.iter().map(|&p| p.into()).collect(),
        };
        let one = payload(&mac, &notices, false, &sub(&["x", "b"])).unwrap();
        assert_eq!(
            serde_json::to_value(&one).unwrap(),
            json!({"type": "notice", "machine": "m", "mac": null, "sealed": {"phone": "b", "data": "for b"}})
        );
        let several = payload(&mac, &notices, true, &sub(&["a"])).unwrap();
        assert_eq!(serde_json::to_value(&several).unwrap()["mac"], "Work Mac");
        assert_eq!(payload(&mac, &notices, false, &sub(&["x"])), None);
    }
}
