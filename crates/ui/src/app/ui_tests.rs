use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use rune::{App, AppSpec, ElementInfo, Env, Runtime, TextEdit};

use super::*;
use crate::api::{Api, Shared};
use crate::data::{Message, MessageStatus, Part, Role};

fn runtime(width: f32, dark: bool, window: Window) -> (tokio::runtime::Runtime, Runtime<State>) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let path = std::env::temp_dir().join(format!("ludolph-ui-{}", uuid::Uuid::new_v4()));
    let shared = Shared::start(
        rt.handle().clone(),
        path.join("engine.sock"),
        path.join("ui.json"),
    );
    let app = App::new(AppSpec::new(
        "test",
        (width, 620.0),
        || unreachable!(),
        crate::view::view,
    ));
    let api = Api::start(&shared, app.handle(), app.handle());
    let state = State::new(
        api,
        window,
        Rc::new(RefCell::new(crate::prefs::Prefs::default())),
        Rc::new(Cell::new(None)),
        Rc::new(Cell::new(false)),
        Rc::new(OnceCell::new()),
    );
    let runtime = Runtime::new(
        state,
        crate::view::view,
        (width, 620.0),
        Env::new(dark, 1.0),
    );
    (rt, runtime)
}

fn settle(runtime: &mut Runtime<State>) {
    for _ in 0..30 {
        runtime.frame(1.0 / 30.0);
    }
}

fn click(runtime: &mut Runtime<State>, element: &ElementInfo) {
    let [x, y, w, h] = element.rect;
    runtime.pointer_down(x + w / 2.0, y + h / 2.0);
    runtime.pointer_up(x + w / 2.0, y + h / 2.0);
    settle(runtime);
}

#[test]
fn chat_code_cards_wrap_independently_and_copy_without_changing_source() {
    let source = format!(
        "Before\n\n```python\nprint('{}')\n```\n\nAfter\n\n```bash\necho done\n```",
        "x".repeat(100)
    );
    for (width, window) in [(420.0, Window::Quick), (1120.0, Window::Main)] {
        for dark in [false, true] {
            let mut prose_height = None;
            for role in [Role::User, Role::Assistant] {
                let (_rt, mut ui) = runtime(width, dark, window);
                let mut chat = Existing::new("test".into(), None);
                chat.page = Some(vec![Message {
                    id: "message".into(),
                    role,
                    created_at: 0,
                    ended_at: 0,
                    status: MessageStatus::Complete,
                    parts: vec![Part::Text {
                        text: source.clone(),
                    }],
                }]);
                ui.state.page = Page::Chat(Box::new(ChatPage::new(
                    0,
                    Target::Existing(chat),
                    TextEdit::default(),
                )));
                settle(&mut ui);
                let elements = ui.elements();
                let card = elements.iter().find(|e| e.role == "code").unwrap();
                assert_eq!(card.radius, 18.0);
                assert!(
                    card.rect[0] >= 0.0 && card.rect[0] + card.rect[2] <= width,
                    "width={width}, dark={dark}, role={role:?}, card={:?}",
                    card.rect
                );
                assert_eq!(elements.iter().filter(|e| e.role == "code").count(), 2);
                assert!(elements.iter().any(|e| e.text.as_deref() == Some("Python")));
                let before = elements
                    .iter()
                    .find(|e| e.text.as_deref() == Some("Before"))
                    .unwrap();
                if let Some(height) = prose_height.replace(before.rect[3]) {
                    assert!(
                        (height - before.rect[3]).abs() < 0.5,
                        "user and agent prose should have the same line height"
                    );
                }
                assert!(elements.iter().any(|e| e.text.as_deref() == Some("After")));
                let old_height = card.rect[3];
                let clipboard = rune::clipboard::memory();
                ui.use_clipboard(&clipboard);
                let wrap = elements
                    .iter()
                    .find(|e| e.label.as_deref() == Some("Enable word wrap"))
                    .unwrap();
                click(&mut ui, wrap);
                let elements = ui.elements();
                assert!(
                    elements
                        .iter()
                        .any(|e| e.label.as_deref() == Some("Disable word wrap"))
                );
                assert!(elements.iter().find(|e| e.role == "code").unwrap().rect[3] > old_height);
                assert!(
                    elements
                        .iter()
                        .any(|e| e.label.as_deref() == Some("Enable word wrap"))
                );
                let copy = elements
                    .iter()
                    .find(|e| e.label.as_deref() == Some("Copy code"))
                    .unwrap();
                click(&mut ui, copy);
                assert!(
                    ui.elements()
                        .iter()
                        .any(|e| e.label.as_deref() == Some("Copied"))
                );
                assert_eq!(
                    clipboard.text().map(|text| text.to_string()),
                    Some(format!("print('{}')", "x".repeat(100)))
                );
                let chat = ui.state.chat().unwrap().existing().unwrap();
                assert_eq!(chat.messages().next().unwrap().text(), source);
            }
        }
    }
}

#[test]
fn reasoning_menu_fills_its_popup_and_stop_has_no_background() {
    let (_rt, mut ui) = runtime(1120.0, true, Window::Main);
    ui.state.catalog = Catalog::Ready(serde_json::from_value(serde_json::json!({
        "models":[{"id":"test/model", "label":"Test", "reasoningLevels":["minimal", "low", "medium", "high", "xhigh", "max"]}],
        "default":"test/model", "defaultReasoning":"high"
    })).unwrap());
    let mut chat = Existing::new(
        "test".into(),
        Some(
            serde_json::from_value(serde_json::json!({
                "id":"test", "status":"working", "cwd":"/tmp", "createdAt":"2026-01-01T00:00:00Z"
            }))
            .unwrap(),
        ),
    );
    chat.page = Some(vec![]);
    ui.state.page = Page::Chat(Box::new(ChatPage::new(
        0,
        Target::Existing(chat),
        TextEdit::default(),
    )));
    settle(&mut ui);
    let elements = ui.elements();
    let stop = elements
        .iter()
        .find(|e| e.label.as_deref() == Some("Stop"))
        .unwrap();
    assert_eq!(stop.fill, rune::Color::TRANSPARENT);
    let [x, y, w, h] = stop.rect;
    ui.pointer_move(x + w / 2.0, y + h / 2.0);
    settle(&mut ui);
    let elements = ui.elements();
    assert_eq!(
        elements
            .iter()
            .find(|e| e.label.as_deref() == Some("Stop"))
            .unwrap()
            .fill,
        rune::Color::TRANSPARENT
    );
    let reason = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("composer-reasoning-trigger"))
        .unwrap();
    click(&mut ui, reason);
    let elements = ui.elements();
    let option = elements
        .iter()
        .find(|e| e.text.as_deref() == Some("Minimal"))
        .unwrap();
    let menu = elements
        .iter()
        .find(|e| e.role == "listbox" && e.rect[1] <= option.rect[1])
        .unwrap();
    let popup = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("composer-reasoning-content"))
        .unwrap();
    assert!(
        (popup.rect[2] - menu.rect[2] - 8.0).abs() < 1.0,
        "menu should fill its popup: popup={:?}, menu={:?}",
        popup.rect,
        menu.rect
    );
    assert!(elements.iter().any(|e| e.text.as_deref() == Some("X-High")));
}

#[test]
fn sent_messages_start_at_the_top_and_hold_while_the_reply_grows() {
    for (width, window) in [(420.0, Window::Quick), (1120.0, Window::Main)] {
        let (_rt, mut ui) = runtime(width, true, window);
        ui.state.engine_up = true;
        let mut chat = Existing::new("test".into(), None);
        chat.page = Some(vec![Message {
            id: "earlier".into(),
            role: Role::Assistant,
            created_at: 0,
            ended_at: 0,
            status: MessageStatus::Complete,
            parts: vec![Part::Text {
                text: "Earlier reply.\n\n".repeat(40),
            }],
        }]);
        ui.state.page = Page::Chat(Box::new(ChatPage::new(
            0,
            Target::Existing(chat),
            TextEdit::new("Next question"),
        )));
        settle(&mut ui);
        ui.update(|s, tx| s.send(tx));
        settle(&mut ui);
        let elements = ui.elements();
        let viewport = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("transcript"))
            .unwrap();
        let question = elements
            .iter()
            .find(|e| e.text.as_deref() == Some("Next question"))
            .unwrap();
        let top = question.rect[1];
        assert!(
            (top - viewport.rect[1] - 18.0).abs() < 1.0,
            "width={width}, top={top}, viewport={:?}",
            viewport.rect
        );
        let at = ui.state.chat().unwrap().existing().unwrap().pending[0].at;
        for lines in [1, 6, 40] {
            ui.update(|s, tx| s.frame(0, serde_json::json!({
                "chat": {"id":"test", "status":"working", "cwd":"/tmp", "createdAt":"2026-01-01T00:00:00Z"},
                "ids":["earlier", "sent", "reply"],
                "messages":[
                    {"id":"sent", "role":"user", "createdAt":at + 100, "endedAt":at + 100, "status":"complete", "parts":[{"kind":"text", "text":"Next question"}]},
                    {"id":"reply", "role":"assistant", "createdAt":at + 101, "endedAt":at + 101, "status":"streaming", "parts":[{"kind":"text", "text":"Reply line.\n\n".repeat(lines)}]}
                ],
                "queued":[], "turnChanges":[], "olderRemaining":0
            }), tx));
            settle(&mut ui);
            let elements = ui.elements();
            let question = elements
                .iter()
                .find(|e| e.text.as_deref() == Some("Next question"))
                .unwrap();
            assert!(
                (question.rect[1] - top).abs() < 1.0,
                "width={width}, lines={lines}, question={:?}",
                question.rect
            );
        }
        let elements = ui.elements();
        let jump = elements
            .iter()
            .find(|e| e.label.as_deref() == Some("Scroll to bottom"))
            .unwrap();
        click(&mut ui, jump);
        assert!(
            !ui.elements()
                .iter()
                .any(|e| e.label.as_deref() == Some("Scroll to bottom"))
        );
        ui.update(|s, tx| {
            s.chat_mut().unwrap().composer = TextEdit::new("Another question");
            s.send(tx);
        });
        settle(&mut ui);
        let elements = ui.elements();
        let question = elements
            .iter()
            .find(|e| e.text.as_deref() == Some("Another question"))
            .unwrap();
        assert!(
            (question.rect[1] - top).abs() < 1.0,
            "the next send must reset the anchor"
        );
    }
}

#[test]
fn notifications_are_rounded_and_centered_at_the_top_in_both_windows() {
    for (width, window) in [(340.0, Window::Quick), (1120.0, Window::Main)] {
        let (_rt, mut ui) = runtime(width, true, window);
        ui.state.notify("Copied");
        settle(&mut ui);
        let elements = ui.elements();
        let toast = elements
            .iter()
            .find(|e| e.role == "status" && e.label.as_deref() == Some("Copied"))
            .unwrap();
        assert_eq!(toast.radius, 18.0);
        assert!((toast.rect[0] + toast.rect[2] / 2.0 - width / 2.0).abs() < 1.0);
        assert!((toast.rect[1] - 16.0).abs() < 1.0);
        let close = elements
            .iter()
            .find(|e| e.label.as_deref() == Some("Close"))
            .unwrap();
        click(&mut ui, close);
        assert!(ui.state.toast.is_none());
    }
}
