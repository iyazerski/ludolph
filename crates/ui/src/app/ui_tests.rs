use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use rune::{App, AppSpec, ElementInfo, Env, Runtime, TextEdit};

use super::*;
use crate::api::{Api, Shared};
use crate::data::{Message, MessageStatus, Part, Role};

fn runtime(width: f32, window: Window) -> (tokio::runtime::Runtime, Runtime<State>) {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let path = std::env::temp_dir().join(format!("ludolph-ui-{}", uuid::Uuid::new_v4()));
    let shared = Shared::start(
        rt.handle().clone(),
        crate::client::Link::nowhere(),
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
    #[cfg(windows)]
    let state = State {
        wsl_setup: Some(crate::wsl::Status::Ready),
        ..state
    };
    let runtime = Runtime::new(
        state,
        crate::view::view,
        (width, 620.0),
        Env::new(true, 1.0),
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

#[cfg(windows)]
#[test]
fn setup_explains_engine_consent_and_pi_steps() {
    use crate::wsl::Status;
    let (_rt, mut ui) = runtime(1120.0, Window::Main);
    ui.update(|s, _| s.wsl_setup = Some(Status::NeedsEngine("Ubuntu".into())));
    settle(&mut ui);
    let elements = ui.elements();
    assert!(
        elements
            .iter()
            .any(|e| e.label.as_deref() == Some("Install engine"))
    );
    assert!(elements.iter().any(|e| {
        e.text
            .as_deref()
            .is_some_and(|t| t.contains("Ubuntu") && t.contains("keep it in sync"))
    }));
    assert!(ui.state.prefs.borrow().managed_wsl.is_none());

    ui.update(|s, _| s.wsl_setup = Some(Status::NeedsPi("Ubuntu".into())));
    settle(&mut ui);
    let elements = ui.elements();
    for label in [
        "pi install instructions",
        "Open WSL terminal",
        "Check again",
    ] {
        assert!(
            elements.iter().any(|e| e.label.as_deref() == Some(label)),
            "missing setup action: {label}"
        );
    }
}

#[test]
fn chat_code_cards_wrap_independently_and_copy_without_changing_source() {
    let code = format!("print('{}')\n\n\tprint('héllo 世界')", "x".repeat(100));
    let source = format!("Before\n\n```python\n{code}\n```\n\nAfter\n\n```bash\necho done\n```");
    for (width, window) in [(420.0, Window::Quick), (1120.0, Window::Main)] {
        let mut prose_height = None;
        for role in [Role::User, Role::Assistant] {
            let (_rt, mut ui) = runtime(width, window);
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
                "width={width}, role={role:?}, card={:?}",
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
                Some(code.clone())
            );
            let chat = ui.state.chat().unwrap().existing().unwrap();
            assert_eq!(chat.messages().next().unwrap().text(), source);
        }
    }
}

#[test]
fn sent_messages_share_markdown_rendering_while_the_composer_keeps_source() {
    let source = "# Heading\n\nUse $review and $team:lint: with **$bold_skill**, $5, `$literal` and [site $linked](https://example.org).\n\n**Bold** and *italic*, ~~old~~ and `code`.\n\n- First\n- Second\n\n[Example](https://example.com)";
    for (width, window) in [(420.0, Window::Quick), (1120.0, Window::Main)] {
        let mut rendered = None;
        for role in [Role::Assistant, Role::User] {
            let (_rt, mut ui) = runtime(width, window);
            let mut chat = Existing::new("test".into(), None);
            chat.page = Some(vec![Message {
                id: "message".into(),
                role,
                created_at: 0,
                ended_at: 0,
                status: MessageStatus::Complete,
                parts: vec![Part::Text {
                    text: source.into(),
                }],
            }]);
            ui.state.page = Page::Chat(Box::new(ChatPage::new(
                0,
                Target::Existing(chat),
                TextEdit::new(source),
            )));
            settle(&mut ui);
            let elements = ui.elements();
            let composer = elements
                .iter()
                .find(|e| e.key.as_deref() == Some("composer"))
                .unwrap();
            assert_eq!(composer.text.as_deref(), Some(source));
            assert!(ui.find("skill:review").is_some());
            assert!(ui.find("skill:team:lint").is_some());
            assert!(ui.find("skill:bold_skill").is_some());
            for literal in ["literal", "linked", "5"] {
                assert!(ui.find(&format!("skill:{literal}")).is_none());
            }
            let message = elements
                .iter()
                .find(|e| e.key.as_deref() == Some("m-message"))
                .unwrap();
            let text: Vec<_> = elements
                .iter()
                .filter(|e| e.path.starts_with(&message.path))
                .filter_map(|e| e.text.as_deref())
                .collect();
            assert!(
                text.iter()
                    .any(|line| line.starts_with("Bold and italic, old and ")
                        && line.contains("code")
                        && !line.contains('`')),
                "width={width}, role={role:?}: {text:?}"
            );
            for expected in ["Heading", "First", "Second", "Example"] {
                assert!(
                    text.contains(&expected),
                    "width={width}, role={role:?}, missing {expected:?}: {text:?}"
                );
            }
            if let Some(rendered) = &rendered {
                assert_eq!(
                    &text, rendered,
                    "user and agent messages render the same Markdown"
                );
            } else {
                rendered = Some(text.iter().map(|text| text.to_string()).collect::<Vec<_>>());
            }
            let chat = ui.state.chat().unwrap().existing().unwrap();
            assert_eq!(chat.messages().next().unwrap().text(), source);

            let clipboard = rune::clipboard::memory();
            ui.use_clipboard(&clipboard);
            ui.update(|s, tx| {
                s.chat_mut().unwrap().composer.focused = false;
                tx.select_all_text("m-message");
            });
            settle(&mut ui);
            assert!(ui.selected_text().is_some());
            ui.key(rune::KeyEvent {
                key: rune::Key::Command('c'),
                mods: rune::Mods {
                    cmd: true,
                    ..Default::default()
                },
            });
            settle(&mut ui);
            let copied = clipboard.text().unwrap();
            assert!(copied.contains("$review"));
            assert!(!copied.contains("skill:"));

            ui.update(|s, tx| {
                s.engine_up = true;
                s.send(tx);
            });
            settle(&mut ui);
            assert!(
                ui.elements()
                    .iter()
                    .all(|e| e.text.as_deref() != Some(source)),
                "newly sent messages hide Markdown syntax too"
            );
            let chat = ui.state.chat().unwrap().existing().unwrap();
            assert_eq!(chat.pending[0].text, source);
        }
    }
}

#[test]
fn skill_selector_matches_the_composer_width_after_resizing() {
    for (window, widths) in [
        (Window::Quick, [420.0, 340.0, 520.0]),
        (Window::Main, [1120.0, 940.0, 1300.0]),
    ] {
        let (_rt, mut ui) = runtime(widths[0], window);
        let mut chat = Existing::new("test".into(), None);
        chat.page = Some(vec![]);
        let mut page = ChatPage::new(0, Target::Existing(chat), TextEdit::new("$"));
        page.composer.focused = true;
        page.skills.list = SkillList::Ready(vec![crate::data::Skill {
            name: "review".into(),
            description:
                "Review the current changes and check for bugs, regressions and missing tests"
                    .into(),
            path: "/tmp/review/SKILL.md".into(),
        }]);
        ui.state.page = Page::Chat(Box::new(page));
        for width in widths {
            ui.set_viewport((width, 620.0), 1.0);
            settle(&mut ui);
            let elements = ui.elements();
            let composer = elements
                .iter()
                .find(|e| e.key.as_deref() == Some("composer-box"))
                .unwrap();
            let popup = elements
                .iter()
                .find(|e| e.key.as_deref() == Some("skills-content"))
                .unwrap();
            assert!(
                (popup.rect[2] - composer.rect[2]).abs() < 1.0,
                "width={width}, popup={:?}, composer={:?}",
                popup.rect,
                composer.rect
            );
            assert!((popup.rect[0] - composer.rect[0]).abs() < 1.0);
        }
        ui.key(rune::Key::Enter.into());
        settle(&mut ui);
        assert_eq!(ui.state.chat().unwrap().composer.text, "$review ");
    }
}

#[test]
fn pasted_web_link_opens_in_the_composer_without_rewriting_text() {
    let url = "https://example.com/a_(b)?one=1&two=2";
    let (_rt, mut ui) = runtime(1120.0, Window::Main);
    let mut chat = Existing::new("test".into(), None);
    chat.page = Some(vec![]);
    ui.state.page = Page::Chat(Box::new(ChatPage::new(
        0,
        Target::Existing(chat),
        TextEdit::default(),
    )));
    settle(&mut ui);
    let composer = ui
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("composer"))
        .unwrap();
    click(&mut ui, &composer);
    ui.paste(url);
    settle(&mut ui);
    let open = ui
        .elements()
        .into_iter()
        .find(|e| e.label.as_deref() == Some("Open link"))
        .expect("a pasted URL can be opened from the composer");
    click(&mut ui, &open);
    assert_eq!(ui.take_url_requests(), [url]);
    assert_eq!(ui.state.chat().unwrap().composer.text, url);
}

#[test]
fn user_message_opens_bare_url_without_prose_punctuation() {
    let url = "https://example.com/a_(b)?one=1&two=2";
    let source = format!("See ({url}). Or run `curl https://example.org`.");
    let (_rt, mut ui) = runtime(1120.0, Window::Main);
    let mut chat = Existing::new("test".into(), None);
    chat.page = Some(vec![Message {
        id: "message".into(),
        role: Role::User,
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
    let (x, y) = ui.find(url).expect("the sent URL is linked in place");
    ui.pointer_down(x, y);
    ui.pointer_up(x, y);
    settle(&mut ui);
    assert_eq!(ui.take_url_requests(), [url]);
    assert!(
        ui.find("https://example.org").is_none(),
        "code stays literal"
    );
    let chat = ui.state.chat().unwrap().existing().unwrap();
    assert_eq!(chat.messages().next().unwrap().text(), source);
}

#[test]
fn reasoning_menu_fills_its_popup_and_stop_has_no_background() {
    let (_rt, mut ui) = runtime(1120.0, Window::Main);
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
fn sent_messages_start_at_the_top_then_follow_the_reply_unless_scrolled_up() {
    for (width, window) in [(420.0, Window::Quick), (1120.0, Window::Main)] {
        let (_rt, mut ui) = runtime(width, window);
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
        let reply = |ui: &mut Runtime<State>, lines: usize| {
            ui.update(|s, tx| s.frame(0, serde_json::json!({
                "chat": {"id":"test", "status":"working", "cwd":"/tmp", "createdAt":"2026-01-01T00:00:00Z"},
                "ids":["earlier", "sent", "reply"],
                "messages":[
                    {"id":"sent", "role":"user", "createdAt":at + 100, "endedAt":at + 100, "status":"complete", "parts":[{"kind":"text", "text":"Next question"}]},
                    {"id":"reply", "role":"assistant", "createdAt":at + 101, "endedAt":at + 101, "status":"streaming", "parts":[{"kind":"text", "text":"Reply line.\n\n".repeat(lines)}]}
                ],
                "queued":[], "turnChanges":[], "olderRemaining":0
            }), tx));
            settle(ui);
            ui.elements()
                .into_iter()
                .find(|e| e.text.as_deref() == Some("Next question"))
                .unwrap()
                .rect[1]
        };
        for lines in [1, 6] {
            let question = reply(&mut ui, lines);
            assert!(
                (question - top).abs() < 1.0,
                "a reply that fits holds the prompt: width={width}, lines={lines}, question={question}"
            );
        }
        assert!(reply(&mut ui, 40) < top - 100.0, "width={width}");
        assert_eq!(
            ui.scroll_at_bottom("transcript"),
            Some(true),
            "width={width}"
        );
        let [x, y, w, h] = viewport.rect;
        ui.wheel(x + w / 2.0, y + h / 2.0, 300.0);
        settle(&mut ui);
        let read = reply(&mut ui, 40);
        assert!(
            (reply(&mut ui, 60) - read).abs() < 1.0,
            "growth doesn't move a chat scrolled up: width={width}"
        );
        let elements = ui.elements();
        let jump = elements
            .iter()
            .find(|e| e.label.as_deref() == Some("Scroll to bottom"))
            .unwrap();
        click(&mut ui, jump);
        reply(&mut ui, 80);
        assert_eq!(
            ui.scroll_at_bottom("transcript"),
            Some(true),
            "width={width}"
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
        let (_rt, mut ui) = runtime(width, window);
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

#[test]
fn closing_the_release_notice_keeps_that_release_from_being_offered_again() {
    let (_rt, mut ui) = runtime(420.0, Window::Quick);
    ui.state.release_notice = Some("9.9.9".into());
    settle(&mut ui);
    let elements = ui.elements();
    let release = elements
        .iter()
        .find(|e| e.role == "status" && e.label.as_deref() == Some("Ludolph 9.9.9 is out"))
        .unwrap();
    assert!((release.rect[1] - 16.0).abs() < 1.0);
    assert!(
        elements
            .iter()
            .any(|e| e.label.as_deref() == Some("Update"))
    );
    let close = elements
        .iter()
        .find(|e| e.label.as_deref() == Some("Close"))
        .unwrap();
    click(&mut ui, close);
    assert!(ui.state.release_notice.is_none());
    assert_eq!(
        ui.state.prefs.borrow().dismissed_release.as_deref(),
        Some("9.9.9")
    );
}

#[test]
fn sent_prompts_rise_into_place_and_keep_their_bubble_once_written() {
    let (_rt, mut ui) = runtime(1120.0, Window::Main);
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
    let prompt = |ui: &Runtime<State>| {
        ui.elements()
            .into_iter()
            .find(|e| e.key.as_deref().is_some_and(|k| k.starts_with("prompt-")))
            .map(|e| (e.rect[1], e.opacity))
            .unwrap()
    };
    let mut path = Vec::new();
    for _ in 0..6 {
        ui.frame(1.0 / 120.0);
        path.push(prompt(&ui));
    }
    let at = ui.state.chat().unwrap().existing().unwrap().pending[0].at;
    ui.update(|s, tx| s.frame(0, serde_json::json!({
        "chat": {"id":"test", "status":"working", "cwd":"/tmp", "createdAt":"2026-01-01T00:00:00Z"},
        "ids":["earlier", "sent"],
        "messages":[
            {"id":"sent", "role":"user", "createdAt":at + 100, "endedAt":at + 100, "status":"complete", "parts":[{"kind":"text", "text":"Next question"}]}
        ],
        "queued":[], "turnChanges":[], "olderRemaining":0
    }), tx));
    ui.frame(1.0 / 120.0);
    path.push(prompt(&ui));
    settle(&mut ui);
    let (top, opacity) = prompt(&ui);
    assert_eq!(opacity, 1.0);
    assert!(
        path.windows(2)
            .all(|w| w[1].0 <= w[0].0 && w[1].1 >= w[0].1),
        "it rises and fades in without jumping back, pi's copy included: {path:?}"
    );
    let (start, faded) = path[1];
    assert!(
        start > top + 1.0 && faded < 1.0,
        "it starts below and faded: {path:?}"
    );
    assert!(path[6].1 < 1.0, "pi's copy carries on the rise: {path:?}");
}

#[test]
fn pages_swap_without_showing_through_or_sliding() {
    type Change = fn(&mut State);
    let sequences: [&[Change]; 4] = [
        &[|s| s.open_changes("0"), State::back],
        &[State::open_settings, State::close_settings],
        &[
            State::open_settings,
            |s| s.show_settings_section(Section::Appearance),
            |s| s.show_settings_section(Section::Model),
            |s| s.show_settings_section(Section::Resources),
            |s| s.show_settings_section(Section::Phone),
            State::close_settings,
        ],
        &[
            |s| s.open_chat("other".into()),
            State::new_chat,
            |s| s.open_chat("test".into()),
        ],
    ];
    for sequence in sequences {
        let (_rt, mut ui) = runtime(1120.0, Window::Main);
        ui.state.engine_up = true;
        ui.state.prefs.borrow_mut().show_details = true;
        let mut chat = Existing::new("test".into(), None);
        chat.page = Some(vec![Message {
            id: "earlier".into(),
            role: Role::Assistant,
            created_at: 0,
            ended_at: 0,
            status: MessageStatus::Complete,
            parts: vec![Part::Text {
                text: "Earlier reply.".into(),
            }],
        }]);
        ui.state.page = Page::Chat(Box::new(ChatPage::new(
            0,
            Target::Existing(chat),
            TextEdit::default(),
        )));
        settle(&mut ui);
        for &change in sequence {
            ui.update(move |s, tx| {
                tx.animated();
                change(s);
            });
            ui.frame(1.0 / 120.0);
            let elements = ui.elements();
            let page = elements
                .iter()
                .find(|e| e.key.as_deref().is_some_and(|k| k.ends_with("-page")))
                .unwrap();
            assert!(page.opacity < 1.0, "the new page {:?} fades in", page.key);
            for e in elements.iter().filter(|e| e.path.starts_with(&page.path)) {
                let moved = (e.rect[0] - e.target[0])
                    .abs()
                    .max((e.rect[1] - e.target[1]).abs());
                assert!(
                    moved < 0.5,
                    "{:?} ({:?}) slides into place inside {:?}",
                    e.key,
                    e.text,
                    page.key
                );
            }
            let exiting: Vec<_> = elements
                .iter()
                .filter(|e| e.exiting)
                .map(|e| (&e.key, e.role, e.opacity, &e.path))
                .collect();
            assert!(
                exiting.is_empty(),
                "the old page stays on screen: {exiting:?}"
            );
            settle(&mut ui);
        }
    }
}

fn test_settings() -> crate::data::Settings {
    crate::data::Settings {
        sidecar_model: None,
        notify: crate::data::Notify::Away,
        open_at_login: false,
        expand_tools: false,
        send_behavior: crate::data::SendBehavior::Queue,
        tool_routing: true,
        second_opinion: false,
        second_opinion_claude: None,
        second_opinion_gpt: None,
        browser_use: false,
        computer_use: false,
        gui_model: None,
        gui_thinking: None,
        chat_models: vec![],
    }
}

#[test]
fn model_and_resource_settings_group_items_and_scroll_long_model_lists() {
    let (_rt, mut ui) = runtime(1120.0, Window::Main);
    ui.update(|s, _| {
        s.open_settings();
        s.show_settings_section(Section::Model);
        s.catalog = Catalog::Ready(
            serde_json::from_value(serde_json::json!({
                "models": [
                    {"id": "anthropic/claude-1", "label": "Claude 1", "reasoningLevels": ["medium"]},
                    {"id": "anthropic/claude-2", "label": "Claude 2", "reasoningLevels": ["medium"]},
                    {"id": "anthropic/claude-3", "label": "Claude 3", "reasoningLevels": ["medium"]},
                    {"id": "anthropic/claude-4", "label": "Claude 4", "reasoningLevels": ["medium"]},
                    {"id": "anthropic/claude-5", "label": "Claude 5", "reasoningLevels": ["medium"]},
                    {"id": "anthropic/claude-6", "label": "Claude 6", "reasoningLevels": ["medium"]},
                    {"id": "anthropic/claude-7", "label": "Claude 7", "reasoningLevels": ["medium"]},
                    {"id": "openai/gpt-1", "label": "GPT 1", "reasoningLevels": ["medium"]},
                    {"id": "openai/gpt-2", "label": "GPT 2", "reasoningLevels": ["medium"]}
                ],
                "default": "anthropic/claude-1",
                "defaultReasoning": "medium"
            }))
            .unwrap(),
        );
        s.settings_page_mut().unwrap().settings = Some(test_settings());
    });
    settle(&mut ui);

    let elements = ui.elements();
    let anthropic_card = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("chat-models-anthropic"))
        .expect("anthropic models grouped in a card");
    let openai_card = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("chat-models-openai"))
        .expect("openai models grouped in a card");
    let anthropic_scroll = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("chat-models-scroll-anthropic"))
        .expect("provider with more than 5 models gets a scrollable list");
    assert!(
        elements
            .iter()
            .all(|e| e.key.as_deref() != Some("chat-models-scroll-openai")),
        "provider with 5 or fewer models does not get a nested scroll list"
    );
    assert!(
        anthropic_scroll.rect[3] < 6.0 * 45.0,
        "provider scroll list shows five and a half rows: {:?}",
        anthropic_scroll.rect
    );
    assert!(elements.iter().any(|e| {
        e.path.starts_with(&anthropic_card.path) && e.text.as_deref() == Some("7 of 7")
    }));
    assert!(
        elements.iter().any(|e| {
            e.path.starts_with(&openai_card.path) && e.text.as_deref() == Some("2 of 2")
        })
    );

    // Scroll the settings page down so the anthropic list is in view, then wheel inside it.
    let page_scroll = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("settings-model-page"))
        .unwrap();
    let [px, py, pw, ph] = page_scroll.rect;
    ui.wheel(px + pw / 2.0, py + ph / 2.0, -400.0);
    settle(&mut ui);
    let elements = ui.elements();
    let anthropic_scroll = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("chat-models-scroll-anthropic"))
        .unwrap();
    let seventh_before = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("anthropic/claude-7"))
        .unwrap()
        .rect[1];
    let [sx, sy, sw, sh] = anthropic_scroll.rect;
    ui.wheel(sx + sw / 2.0, sy + sh / 2.0, -200.0);
    settle(&mut ui);
    let seventh_after = ui
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("anthropic/claude-7"))
        .unwrap()
        .rect[1];
    assert!(
        seventh_after < seventh_before - 10.0,
        "scrolling inside the provider list moves later models into view: {seventh_before} -> {seventh_after}"
    );

    // Skills and extensions group by source.
    ui.update(|s, _| {
        s.show_settings_section(Section::Resources);
        s.settings_page_mut().unwrap().resources = Some(Ok(vec![
            crate::data::Resource {
                kind: crate::data::ResourceKind::Skills,
                name: "review".into(),
                path: "/builtin/review/SKILL.md".into(),
                group: "Built-in".into(),
                enabled: true,
            },
            crate::data::Resource {
                kind: crate::data::ResourceKind::Skills,
                name: "commit".into(),
                path: "/builtin/commit/SKILL.md".into(),
                group: "Built-in".into(),
                enabled: false,
            },
            crate::data::Resource {
                kind: crate::data::ResourceKind::Skills,
                name: "deploy".into(),
                path: "/home/.agents/skills/deploy/SKILL.md".into(),
                group: "~/.agents/skills".into(),
                enabled: true,
            },
            crate::data::Resource {
                kind: crate::data::ResourceKind::Extensions,
                name: "browse".into(),
                path: "/builtin/browse.ts".into(),
                group: "Built-in".into(),
                enabled: true,
            },
        ]));
    });
    settle(&mut ui);
    let elements = ui.elements();
    let builtin_skills = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("resource-skills-Built-in"))
        .expect("Built-in skills grouped into their own card");
    let local_skills = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("resource-skills-~/.agents/skills"))
        .expect("local skills grouped into their own card");
    assert!(elements.iter().any(|e| {
        e.path.starts_with(&builtin_skills.path) && e.text.as_deref() == Some("1 of 2")
    }));
    assert!(elements.iter().any(|e| {
        e.path.starts_with(&local_skills.path) && e.text.as_deref() == Some("deploy")
    }));
    assert!(
        elements
            .iter()
            .any(|e| e.key.as_deref() == Some("resource-extensions-Built-in"))
    );
}

#[test]
fn phone_settings_separates_relay_linked_phones_and_notifications() {
    let (_rt, mut ui) = runtime(1120.0, Window::Main);
    ui.update(|s, _| {
        s.open_settings();
        s.show_settings_section(Section::Phone);
        let page = s.settings_page_mut().unwrap();
        page.settings = Some(test_settings());
        page.phone.state = Some(crate::data::PhoneLink {
            link: Some(crate::data::RelayLink {
                relay: "https://ludolph.iyazerski.dev".into(),
                status: crate::data::LinkStatus::Connected,
                error: None,
            }),
            phones: vec!["T01VVtGD123456".into()],
            pairing: None,
            relay: "https://ludolph.iyazerski.dev".into(),
        });
    });
    settle(&mut ui);

    let elements = ui.elements();
    assert!(
        elements
            .iter()
            .any(|e| e.text.as_deref() == Some("Relay connected"))
    );
    assert!(
        elements
            .iter()
            .any(|e| e.text.as_deref() == Some("ludolph.iyazerski.dev"))
    );
    assert!(
        elements
            .iter()
            .any(|e| e.text.as_deref() == Some("Linked phones"))
    );
    assert!(
        elements
            .iter()
            .any(|e| e.label.as_deref() == Some("Link phone"))
    );
    assert!(
        elements
            .iter()
            .any(|e| e.text.as_deref() == Some("Phone T01VVtGD"))
    );
    assert!(
        elements
            .iter()
            .any(|e| e.text.as_deref() == Some("Notifications"))
    );
    assert!(
        elements
            .iter()
            .any(|e| e.text.as_deref() == Some("Send notifications"))
    );

    let unlink = elements
        .iter()
        .find(|e| e.label.as_deref() == Some("Unlink"))
        .unwrap();
    click(&mut ui, unlink);
    let elements = ui.elements();
    assert!(
        elements
            .iter()
            .any(|e| e.text.as_deref() == Some("Unlink Phone T01VVtGD?"))
    );
    let cancel = elements
        .iter()
        .find(|e| e.label.as_deref() == Some("Cancel"))
        .unwrap();
    click(&mut ui, cancel);
    assert!(
        ui.elements()
            .iter()
            .any(|e| e.text.as_deref() == Some("Phone T01VVtGD"))
    );

    // Empty state shows a dedicated empty card with the primary Link phone button.
    ui.update(|s, _| {
        s.settings_page_mut()
            .unwrap()
            .phone
            .state
            .as_mut()
            .unwrap()
            .phones
            .clear();
    });
    settle(&mut ui);
    let elements = ui.elements();
    assert!(
        elements
            .iter()
            .any(|e| e.text.as_deref() == Some("No phones linked"))
    );
    assert!(
        elements
            .iter()
            .any(|e| e.label.as_deref() == Some("Link phone"))
    );
}
