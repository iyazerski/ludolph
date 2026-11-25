use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use rune::{App, AppSpec, ElementInfo, Env, Runtime, TextEdit};

use super::*;
use crate::api::{Api, Shared};
use crate::data::{Message, MessageStatus, Part, Role, ToolState};

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
    let text =
        rune::text::TextSystem::new(rune_theme_opencode::FONTS, rune_theme_opencode::FAMILIES);
    let runtime = Runtime::with_text(
        state,
        crate::view::view,
        (width, 620.0),
        Env::new(true, 1.0),
        text,
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
fn model_selector_unifies_model_and_reasoning_in_main_and_quick_windows() {
    for (width, window) in [(420.0, Window::Quick), (1120.0, Window::Main)] {
        let (_rt, mut ui) = runtime(width, window);
        ui.state.catalog = Catalog::Ready(serde_json::from_value(serde_json::json!({
            "models":[
                {"id":"test/model", "label":"Test", "reasoningLevels":["minimal", "low", "medium", "high", "xhigh", "max"]},
                {"id":"openai/gpt-5", "label":"GPT-5", "reasoningLevels":["low", "medium", "high"]}
            ],
            "default":"test/model", "defaultReasoning":"high"
        })).unwrap());
        let mut chat = Existing::new(
            "test".into(),
            Some(
                serde_json::from_value(serde_json::json!({
                    "id":"test", "status":"idle", "cwd":"/tmp", "createdAt":"2026-01-01T00:00:00Z", "reasoning":"high"
                }))
                .unwrap(),
            ),
        );
        chat.page = Some(vec![]);
        let mut page = ChatPage::new(0, Target::Existing(chat), TextEdit::default());
        page.info = Some(
            serde_json::from_value(serde_json::json!({
                "model": "test/model",
                "reasoning": "high"
            }))
            .unwrap(),
        );
        ui.state.page = Page::Chat(Box::new(page));
        settle(&mut ui);

        // Closed state: single trigger shows model + reasoning, no separate inline slider.
        let elements = ui.elements();
        assert!(
            elements.iter().all(|e| e.role != "slider"),
            "no separate inline reasoning slider when closed: width={width}"
        );
        let trigger = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("composer-model-trigger"))
            .unwrap();
        assert_eq!(trigger.role, "combobox");
        assert!(
            elements.iter().any(|e| {
                e.key.as_deref() == Some("composer-model-trigger-effort")
                    && e.text.as_deref() == Some("High")
            }),
            "trigger shows active reasoning effort: width={width}"
        );

        // Open the unified selector: shows compact effort card with header + pill slider.
        click(&mut ui, trigger);
        let elements = ui.elements();
        let slider = elements
            .iter()
            .find(|e| e.key.as_deref() == Some("composer-model-reasoning"))
            .expect("effort card mounts pill reasoning slider");
        assert_eq!(slider.role, "slider");
        let [sx, sy, sw, sh] = slider.rect;
        // Drag to stop 4 of 5 ("xhigh"): pad = 13, travel = sw - 26.
        let target_x = sx + 13.0 + (sw - 26.0) * (4.0 / 5.0);
        let mid_y = sy + sh / 2.0;
        ui.pointer_move(sx + 13.0, mid_y);
        ui.pointer_down(sx + 13.0, mid_y);
        ui.pointer_move(target_x, mid_y);
        settle(&mut ui);
        assert!(
            ui.elements()
                .iter()
                .any(|e| e.key.as_deref() == Some("composer-model-effort")
                    && e.text.as_deref() == Some("X-High")),
            "dragging previews the targeted reasoning level in the card header"
        );
        ui.pointer_up(target_x, mid_y);
        settle(&mut ui);
        assert_eq!(ui.state.config().reasoning, Some("xhigh"));

        // Clicking the card header drills into the searchable model list.
        let header = ui
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("composer-model-header"))
            .unwrap();
        click(&mut ui, &header);
        let gpt = ui
            .elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some("composer-model-option-openai/gpt-5"))
            .expect("model list shows available models");
        click(&mut ui, &gpt);
        assert_eq!(ui.state.config().model, Some("openai/gpt-5"));
        assert!(
            ui.elements()
                .iter()
                .any(|e| e.key.as_deref() == Some("composer-model-reasoning")),
            "picking a reasoning model returns to the effort card"
        );

        // Escape closes the open selector without hiding the quick window.
        let esc = rune::KeyEvent {
            key: rune::Key::Escape,
            mods: rune::Mods::default(),
        };
        ui.update(|s, tx| {
            assert!(crate::view::on_key(s, &esc, tx));
        });
        settle(&mut ui);
        assert!(
            ui.elements()
                .iter()
                .all(|e| e.key.as_deref() != Some("composer-model-content"))
        );
    }
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
fn release_notice_is_one_line_dismissible_and_updates_on_about() {
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
        release.rect[3] <= 44.0,
        "release notice should be one line: {:?}",
        release.rect
    );
    assert!(
        !elements
            .iter()
            .any(|e| e.text.as_deref().is_some_and(|t| t.contains("restarts"))),
        "release notice should omit the description"
    );
    let update = elements
        .iter()
        .find(|e| e.label.as_deref() == Some("Update"))
        .unwrap();
    let close = elements
        .iter()
        .find(|e| e.label.as_deref() == Some("Close"))
        .unwrap();
    let center_y = |e: &ElementInfo| e.rect[1] + e.rect[3] / 2.0;
    assert!(
        (center_y(update) - center_y(close)).abs() < 1.0
            && (center_y(update) - center_y(release)).abs() < 1.0,
        "title, update button and close button share one line: release={:?} update={:?} close={:?}",
        release.rect,
        update.rect,
        close.rect
    );
    click(&mut ui, close);
    assert!(ui.state.release_notice.is_none());
    assert_eq!(
        ui.state.prefs.borrow().dismissed_release.as_deref(),
        Some("9.9.9")
    );

    let (_rt, mut main) = runtime(1120.0, Window::Main);
    main.state.prefs.borrow_mut().release_checked = Some(chrono::Utc::now());
    main.state.release_notice = Some("0.0.1".into());
    settle(&mut main);
    let update = main
        .elements()
        .into_iter()
        .find(|e| e.label.as_deref() == Some("Update"))
        .unwrap();
    click(&mut main, &update);
    assert!(main.state.release_notice.is_none());
    assert_eq!(
        main.state.settings_page().map(|p| p.section),
        Some(Section::About)
    );
}

#[test]
fn updated_notice_opens_github_release_notes_and_dismisses() {
    let (_rt, mut ui) = runtime(420.0, Window::Quick);
    ui.state.updated_notice = Some("0.2.19".into());
    settle(&mut ui);
    let elements = ui.elements();
    let notice = elements
        .iter()
        .find(|e| e.role == "status" && e.label.as_deref() == Some("Ludolph updated to 0.2.19"))
        .unwrap();
    assert!((notice.rect[1] - 16.0).abs() < 1.0);
    assert!(
        notice.rect[3] <= 44.0,
        "updated notice should be one line: {:?}",
        notice.rect
    );
    let notes = elements
        .iter()
        .find(|e| e.label.as_deref() == Some("Release notes"))
        .unwrap();
    let close = elements
        .iter()
        .find(|e| e.label.as_deref() == Some("Close"))
        .unwrap();
    let center_y = |e: &ElementInfo| e.rect[1] + e.rect[3] / 2.0;
    assert!((center_y(notes) - center_y(close)).abs() < 1.0);
    assert!(
        notice.rect[0] >= 15.0 && notice.rect[0] + notice.rect[2] <= 405.0,
        "updated notice fits inside the quick window: {:?}",
        notice.rect
    );

    click(&mut ui, notes);
    assert_eq!(
        ui.take_url_requests(),
        ["https://github.com/iyazerski/ludolph/releases/tag/v0.2.19"]
    );
    assert!(ui.state.updated_notice.is_none());

    ui.update(|s, _| s.updated_notice = Some("0.2.19".into()));
    settle(&mut ui);
    let close = ui
        .elements()
        .into_iter()
        .find(|e| e.label.as_deref() == Some("Close"))
        .unwrap();
    click(&mut ui, &close);
    assert!(ui.state.updated_notice.is_none());
    assert!(ui.take_url_requests().is_empty());
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
    for _ in 0..30 {
        ui.frame(1.0 / 120.0);
        path.push(prompt(&ui));
    }
    settle(&mut ui);
    let (top, opacity) = prompt(&ui);
    assert_eq!(opacity, 1.0);
    assert!(
        path.windows(2)
            .all(|w| w[1].0 <= w[0].0 && w[1].1 >= w[0].1),
        "it scrolls up smoothly without jumping back, pi's copy included: {path:?}"
    );
    assert!(
        path[0].0 > top + 200.0 && path[6].0 > top + 100.0,
        "sending scrolls smoothly from the bottom instead of jumping straight to the top: top={top}, path={:?}",
        &path[..8]
    );
    assert!(
        path[35].0 < top + 40.0,
        "the scroll glide finishes promptly, not slowly: top={top}, at_300ms={}",
        path[35].0
    );

    // A second send with a prior sent-turn also glides smoothly up from below.
    ui.update(|s, tx| {
        s.chat_mut().unwrap().composer = TextEdit::new("Follow-up question");
        s.send(tx);
    });
    let mut second = Vec::new();
    for _ in 0..12 {
        ui.frame(1.0 / 120.0);
        second.push(prompt(&ui).0);
    }
    settle(&mut ui);
    let second_top = prompt(&ui).0;
    assert!(
        second.windows(2).all(|w| w[1] <= w[0]),
        "second send glides up smoothly without jumping back: {second:?}"
    );
    assert!(
        second[0] > second_top + 40.0 && second[11] < second[0] - 20.0,
        "second send visibly scrolls up: top={second_top}, path={second:?}"
    );
}

#[test]
fn reasoning_rows_expand_and_collapse_smoothly_without_jumping_the_icon() {
    let thought_text = (1..=10)
        .map(|i| format!("Reasoning line {i} explaining the step in detail."))
        .collect::<Vec<_>>()
        .join("\n");
    let (_rt, mut ui) = runtime(1120.0, Window::Main);
    ui.state.engine_up = true;
    let mut chat = Existing::new("test".into(), None);
    chat.open_groups.insert("reply#worked".into(), true);
    chat.open_groups.insert("reply#0".into(), true);
    chat.open_groups.insert("solo#worked".into(), true);
    chat.page = Some(vec![
        Message {
            id: "earlier".into(),
            role: Role::Assistant,
            created_at: 0,
            ended_at: 0,
            status: MessageStatus::Complete,
            parts: vec![Part::Text {
                text: "Earlier context.\n\n".repeat(30),
            }],
        },
        Message {
            id: "reply".into(),
            role: Role::Assistant,
            created_at: 1,
            ended_at: 2,
            status: MessageStatus::Complete,
            parts: vec![
                Part::Tool {
                    id: "t1".into(),
                    name: "read".into(),
                    subject: Some("a.rs".into()),
                    state: ToolState::Done,
                    calls: Vec::new(),
                },
                Part::Thinking {
                    text: thought_text.clone(),
                },
                Part::Tool {
                    id: "t2".into(),
                    name: "edit".into(),
                    subject: Some("b.rs".into()),
                    state: ToolState::Done,
                    calls: Vec::new(),
                },
                Part::Text {
                    text: "Done updating the files.".into(),
                },
            ],
        },
        Message {
            id: "solo".into(),
            role: Role::Assistant,
            created_at: 3,
            ended_at: 4,
            status: MessageStatus::Complete,
            parts: vec![
                Part::Thinking { text: thought_text },
                Part::Text {
                    text: "Standalone answer.".into(),
                },
            ],
        },
    ]);
    ui.state.page = Page::Chat(Box::new(ChatPage::new(
        0,
        Target::Existing(chat),
        TextEdit::default(),
    )));
    settle(&mut ui);

    // 1. Reasoning row inside a tool group (with icons::THOUGHT).
    let find_key = |ui: &Runtime<State>, key: &str| {
        ui.elements()
            .into_iter()
            .find(|e| e.key.as_deref() == Some(key))
            .unwrap_or_else(|| panic!("missing element {key}"))
    };
    let thought = find_key(&ui, "thought-reply#0:1");
    let closed_h = thought.rect[3];
    let head_y = thought.rect[1];
    let head = ui
        .elements()
        .into_iter()
        .find(|e| e.path.starts_with(&thought.path) && e.key.as_deref() == Some("head"))
        .unwrap();
    let head_path = format!("{}/head", thought.path);
    let head_sizes: Vec<(String, f32, f32)> = ui
        .elements()
        .into_iter()
        .filter(|e| e.path.starts_with(&head_path))
        .map(|e| (e.path, e.rect[2], e.rect[3]))
        .collect();
    assert!(
        head_sizes
            .iter()
            .any(|(_, w, h)| (*w - 14.0).abs() < 0.5 && (*h - 14.0).abs() < 0.5),
        "thought icon present in head: {head_sizes:?}"
    );
    let click_at = |ui: &mut Runtime<State>, rect: [f32; 4]| {
        let (x, y) = (rect[0] + rect[2] / 2.0, rect[1] + rect[3] / 2.0);
        ui.pointer_down(x, y);
        ui.pointer_up(x, y);
    };
    let sample = |ui: &mut Runtime<State>,
                  key: &str,
                  expected_head: Option<&[(String, f32, f32)]>| {
        let mut frames = Vec::new();
        for _ in 0..36 {
            ui.frame(1.0 / 120.0);
            let els = ui.elements();
            let row = els.iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
            let body_opacity = els
                .iter()
                .find(|e| e.path.starts_with(&row.path) && e.key.as_deref() == Some("body"))
                .map(|e| e.opacity);
            if let Some(expected) = expected_head {
                for (path, w, h) in expected {
                    let el = els
                        .iter()
                        .find(|e| e.path == *path && !e.exiting)
                        .unwrap_or_else(|| panic!("missing head element {path}"));
                    assert!(
                        (el.rect[2] - *w).abs() < 0.5 && (el.rect[3] - *h).abs() < 0.5,
                        "head element {path} resized unexpectedly: expected ({w}, {h}), got {:?}",
                        el.rect
                    );
                }
            }
            frames.push((row.rect[1], row.rect[3], body_opacity));
        }
        frames
    };

    click_at(&mut ui, head.rect);
    let expand = sample(&mut ui, "thought-reply#0:1", Some(&head_sizes));
    settle(&mut ui);
    let open_h = find_key(&ui, "thought-reply#0:1").rect[3];
    assert!(
        open_h > closed_h + 100.0,
        "thought expanded: {closed_h} -> {open_h}"
    );
    assert!(
        expand[0].1 < closed_h + (open_h - closed_h) * 0.25,
        "expand animates smoothly instead of jumping open: closed={closed_h}, open={open_h}, frames={:?}",
        &expand[..5]
    );
    assert!(
        expand.windows(2).all(|w| w[1].1 >= w[0].1 - 0.1),
        "expand height increases monotonically: {expand:?}"
    );
    assert!(
        expand[35].1 > closed_h + (open_h - closed_h) * 0.85,
        "expand completes promptly: closed={closed_h}, open={open_h}, at_300ms={}",
        expand[35].1
    );
    assert!(
        expand.iter().all(|f| (f.0 - head_y).abs() < 1.5),
        "header stays under the cursor while expanding: head_y={head_y}, frames={expand:?}"
    );

    // Click again to collapse: icon stays 14x14 and height/opacity reverse expand.
    let head = ui
        .elements()
        .into_iter()
        .find(|e| e.path.starts_with(&thought.path) && e.key.as_deref() == Some("head"))
        .unwrap();
    click_at(&mut ui, head.rect);
    let collapse = sample(&mut ui, "thought-reply#0:1", Some(&head_sizes));
    settle(&mut ui);
    assert!(
        (find_key(&ui, "thought-reply#0:1").rect[3] - closed_h).abs() < 0.5,
        "thought returned to closed height"
    );
    assert!(
        collapse[0].1 > open_h - (open_h - closed_h) * 0.25,
        "collapse animates smoothly instead of jumping shut: closed={closed_h}, open={open_h}, frames={:?}",
        &collapse[..5]
    );
    assert!(
        collapse.windows(2).all(|w| w[1].1 <= w[0].1 + 0.1),
        "collapse height decreases monotonically: {collapse:?}"
    );
    for (i, (e, c)) in expand.iter().zip(&collapse).enumerate() {
        let mirror_h = closed_h + open_h - e.1;
        assert!(
            (c.1 - mirror_h).abs() < 2.0,
            "frame {i}: collapse height {} should mirror expand {} (expected {mirror_h})",
            c.1,
            e.1
        );
        if let (Some(eo), Some(co)) = (e.2, c.2) {
            assert!(
                (eo + co - 1.0).abs() < 0.06,
                "frame {i}: collapse opacity {co} should mirror expand opacity {eo}"
            );
        }
    }

    // 2. Standalone thinking row also expands and collapses symmetrically.
    let solo = find_key(&ui, "thinking-solo#0");
    let solo_closed = solo.rect[3];
    let solo_head = ui
        .elements()
        .into_iter()
        .find(|e| e.path.starts_with(&solo.path) && e.key.as_deref() == Some("head"))
        .unwrap();
    click_at(&mut ui, solo_head.rect);
    let solo_expand = sample(&mut ui, "thinking-solo#0", None);
    settle(&mut ui);
    let solo_open = find_key(&ui, "thinking-solo#0").rect[3];
    assert!(solo_open > solo_closed + 100.0);
    let solo_head = ui
        .elements()
        .into_iter()
        .find(|e| e.path.starts_with(&solo.path) && e.key.as_deref() == Some("head"))
        .unwrap();
    click_at(&mut ui, solo_head.rect);
    let solo_collapse = sample(&mut ui, "thinking-solo#0", None);
    settle(&mut ui);
    assert!((find_key(&ui, "thinking-solo#0").rect[3] - solo_closed).abs() < 0.5);
    for (i, (e, c)) in solo_expand.iter().zip(&solo_collapse).enumerate() {
        let mirror_h = solo_closed + solo_open - e.1;
        assert!(
            (c.1 - mirror_h).abs() < 2.0,
            "solo frame {i}: collapse height {} should mirror expand {} (expected {mirror_h})",
            c.1,
            e.1
        );
    }
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
                    {"id": "anthropic/claude-1", "label": "Claude 1", "reasoningLevels": ["low", "medium", "high"]},
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
        s.settings_page_mut().unwrap().settings = Some(crate::data::Settings {
            sidecar_model: Some("openai-codex/gone".into()),
            ..test_settings()
        });
    });
    settle(&mut ui);

    let elements = ui.elements();
    assert!(
        elements
            .iter()
            .any(|e| e.text.as_deref() == Some("openai-codex/gone (unavailable)")),
        "a saved model pi no longer offers stays visible"
    );
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
    let anthropic_bar = elements
        .iter()
        .find(|e| {
            e.path
                .ends_with("/chat-models-scroll-anthropic-scroll-area/scroll-bar")
        })
        .expect("scrollable provider list has a scrollbar track");
    assert_eq!(anthropic_bar.opacity, 1.0);
    let anthropic_thumb = elements
        .iter()
        .find(|e| {
            e.path
                .ends_with("/chat-models-scroll-anthropic-scroll-area/scroll-bar/thumb-slot/thumb")
        })
        .expect("scrollable provider list has a visible scrollbar thumb");
    assert!(
        anthropic_thumb.rect[2] > 0.0 && anthropic_thumb.rect[3] >= 24.0,
        "scrollbar thumb is sized visibly at rest: {:?}",
        anthropic_thumb.rect
    );
    assert!(
        elements
            .iter()
            .all(|e| e.key.as_deref() != Some("chat-models-scroll-openai")),
        "provider with 5 or fewer models does not get a nested scroll list"
    );
    assert!(
        elements
            .iter()
            .all(|e| e.key.as_deref() != Some("chat-models-scroll-openai-scroll-area")),
        "short provider list has no scrollbar"
    );
    assert!(
        elements.iter().all(|e| e.role != "slider"),
        "settings uses the unified model selector rather than separate inline sliders"
    );
    let claude_trigger = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("review-claude-trigger"))
        .expect("Claude reviewer uses the unified model selector");
    assert_eq!(claude_trigger.role, "combobox");
    click(&mut ui, claude_trigger);
    let claude_opt = ui
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some("review-claude-option-anthropic/claude-1"))
        .expect("clicking unset reviewer opens the searchable model list");
    click(&mut ui, &claude_opt);
    assert_eq!(
        ui.elements()
            .iter()
            .find(|e| e.key.as_deref() == Some("review-claude-reasoning"))
            .expect("picking a reasoning reviewer opens the effort card with its pill slider")
            .role,
        "slider"
    );
    ui.update(|s, _| {
        s.settings_page_mut().unwrap().close_open_picker();
    });
    settle(&mut ui);
    let elements = ui.elements();
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
    let thumb_before = elements
        .iter()
        .find(|e| {
            e.path
                .ends_with("/chat-models-scroll-anthropic-scroll-area/scroll-bar/thumb-slot/thumb")
        })
        .unwrap()
        .rect[1];
    let seventh_before = elements
        .iter()
        .find(|e| e.key.as_deref() == Some("anthropic/claude-7"))
        .unwrap()
        .rect[1];
    let [sx, sy, sw, sh] = anthropic_scroll.rect;
    ui.wheel(sx + sw / 2.0, sy + sh / 2.0, -200.0);
    settle(&mut ui);
    let after = ui.elements();
    let seventh_after = after
        .iter()
        .find(|e| e.key.as_deref() == Some("anthropic/claude-7"))
        .unwrap()
        .rect[1];
    let thumb_after = after
        .iter()
        .find(|e| {
            e.path
                .ends_with("/chat-models-scroll-anthropic-scroll-area/scroll-bar/thumb-slot/thumb")
        })
        .unwrap()
        .rect[1];
    assert!(
        seventh_after < seventh_before - 10.0,
        "scrolling inside the provider list moves later models into view: {seventh_before} -> {seventh_after}"
    );
    assert!(
        thumb_after > thumb_before + 5.0,
        "scrolling moves the visible scrollbar thumb down: {thumb_before} -> {thumb_after}"
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
