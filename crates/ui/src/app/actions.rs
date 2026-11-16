use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use rune::{TextEdit, Tx};
use rune_kit::diff::{self, DiffState};
use serde::Deserialize;
use serde_json::{Value, json};

use super::page::{ChangeTarget, Changes, Dictating, LEVELS_KEPT, SkillList, SkillMenu, Voice};
use super::{ChatPage, Pending, QuestionForm, State, Target, attach};
use crate::data::{
    ChatView, ComposerInfo, Diff, Frame, Message, MessageStatus, Part, Polished, Skills, ToolState,
};
use crate::dictation::{self, Heard, Listening};

const EARLIER: usize = 40;

enum ChatAction {
    Stop,
    Respond(Answer),
}

pub enum Answer {
    Value(String),
    Confirmed(bool),
}

#[derive(Deserialize)]
struct Sent {
    delivery: String,
    text: String,
}

fn finished_in(page: &[Message], call: &str) -> bool {
    page.iter().flat_map(|m| &m.parts).any(|part| {
        matches!(part, Part::Tool { id, state, .. } if id == call && *state != ToolState::Running)
    })
}

impl State {
    pub(super) fn frame(&mut self, nav: u64, record: Value, tx: &mut Tx) {
        let Some(chat) = self.existing_at(nav) else {
            return;
        };
        if let Some(why) = record.get("error") {
            chat.error = Some(why.as_str().unwrap_or("the chat can't be read").to_owned());
            chat.feed = None;
            return;
        }
        let frame: Frame = match serde_json::from_value(record) {
            Ok(frame) => frame,
            Err(e) => {
                chat.error = Some(format!("unexpected chat frame: {e}"));
                return;
            }
        };
        chat.error = frame.failure;
        // Springs would relayout each streaming update at the display refresh rate.
        if !frame.reset
            && frame
                .messages
                .iter()
                .any(|m| m.status == MessageStatus::Streaming)
        {
            tx.instant();
        }
        let mut kept: HashMap<String, Message> = if frame.reset {
            chat.before.clear();
            chat.before_changes.clear();
            HashMap::new()
        } else {
            chat.page
                .take()
                .unwrap_or_default()
                .into_iter()
                .map(|m| (m.id.clone(), m))
                .collect()
        };
        let mut fresh: HashMap<String, Message> = frame
            .messages
            .into_iter()
            .map(|m| (m.id.clone(), m))
            .collect();
        let page: Vec<Message> = frame
            .ids
            .iter()
            .filter_map(|id| fresh.remove(id).or_else(|| kept.remove(id)))
            .collect();
        chat.confirm_pending(&page, frame.run.as_ref());
        if chat.question.as_ref().map(|q| &q.dialog.id)
            != frame.pending_input.as_ref().map(|d| &d.id)
        {
            chat.question = frame.pending_input.map(QuestionForm::new);
            if chat.question.is_some() {
                tx.scroll_to_bottom("transcript");
            }
        }
        if chat.before.is_empty() {
            chat.older_remaining = frame.older_remaining;
        }
        chat.turn_changes = frame.turn_changes;
        chat.run = frame.run;
        chat.queued = frame.queued;
        let finished: Vec<String> = chat
            .tool_outputs
            .iter()
            .filter(|(_, output)| matches!(output, Some(Ok(output)) if output.running))
            .filter(|(call, _)| finished_in(&page, call))
            .map(|(call, _)| call.clone())
            .collect();
        chat.page = Some(page);
        chat.file_links.borrow_mut().clear();
        // A snapshot taken while its tool ran misses the end, often the error or summary.
        for call in &finished {
            chat.tool_outputs.insert(call.clone(), None);
        }
        for call in finished {
            self.fetch_tool_output(nav, call);
        }
        let Some(chat) = self.existing_at(nav) else {
            return;
        };
        let moved = chat
            .chat
            .as_ref()
            .is_some_and(|before| before.cwd != frame.chat.cwd);
        let configured = moved
            || chat.chat.as_ref().is_some_and(|before| {
                before.model != frame.chat.model || before.reasoning != frame.chat.reasoning
            });
        let working = frame.chat.is_working();
        let settled = chat.chat.as_ref().is_some_and(|c| c.is_working()) && !working;
        let seen = self.prefs.borrow_mut().mark_seen(&frame.chat);
        if seen {
            self.save_prefs();
        }
        if let Some(chat) = self.existing_at(nav) {
            let due = configured || settled || (working && chat.info_due());
            if configured || settled || frame.reset {
                chat.details_at = None;
            }
            chat.chat = Some(frame.chat);
            self.load_thumbs();
            if due {
                self.load_info();
            }
        }
        if moved {
            self.reload_skills();
        }
        self.load_details();
        self.sync_tick();
    }

    pub fn take_over(&mut self) {
        let Some(chat) = self.chat().and_then(|page| page.existing()) else {
            return;
        };
        self.api.call(
            "take_over_chat",
            json!({"chat": chat.id}),
            |s, result: Result<Value, String>, _| {
                if let Err(error) = result {
                    s.fail("Couldn't continue here", error);
                }
            },
        );
    }

    pub fn open_changes(&mut self, turn: &str) {
        self.load_changes(ChangeTarget::Turn(turn.to_owned()));
    }

    pub fn open_working_changes(&mut self) {
        self.load_changes(ChangeTarget::WorkingTree);
    }

    fn load_changes(&mut self, target: ChangeTarget) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing_mut() else {
            return;
        };
        chat.changes = Some(Changes::Loading);
        chat.changes_of = target.clone();
        let (method, args) = match &target {
            ChangeTarget::Turn(turn) => ("turn_changes", json!({"chat": chat.id, "turn": turn})),
            ChangeTarget::WorkingTree => ("working_changes", json!({"chat": chat.id})),
        };
        self.api
            .call(method, args, move |s, result: Result<Diff, String>, _| {
                let Some(chat) = s
                    .existing_at(nav)
                    .filter(|c| c.changes.is_some() && c.changes_of == target)
                else {
                    return;
                };
                chat.changes = Some(match result {
                    Ok(got) => {
                        let mut diff = Box::new(DiffState::new(diff::parse(&got.patch)));
                        diff.set_wrap(true);
                        Changes::Ready {
                            diff,
                            truncated: got.truncated,
                        }
                    }
                    Err(e) => Changes::Failed(e),
                });
            });
    }

    pub fn load_earlier(&mut self) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing_mut().filter(|c| !c.loading_earlier) else {
            return;
        };
        let Some(first) = chat.messages().next().map(|m| m.id.clone()) else {
            return;
        };
        chat.loading_earlier = true;
        let args = json!({"chat": chat.id, "from": first, "earlier": EARLIER, "thinking": true});
        self.api.call(
            "chat_view",
            args,
            move |s, result: Result<ChatView, String>, tx| {
                let Some(chat) = s.existing_at(nav) else {
                    return;
                };
                chat.loading_earlier = false;
                let read = match result {
                    Ok(ChatView { read: Some(read) }) => read,
                    Ok(ChatView { read: None }) => return,
                    Err(e) => return s.fail("Couldn't load earlier messages", e),
                };
                let Some(at) = read.messages.iter().position(|m| m.id == first) else {
                    return;
                };
                let mut earlier = read.messages;
                earlier.truncate(at);
                earlier.append(&mut chat.before);
                chat.before = earlier;
                chat.older_remaining = read.older_remaining;
                chat.before_changes = read.turn_changes;
                // Scroll anchoring lets content added at the very top show, and the
                // reader was at the top to press the button: keep their place instead.
                tx.keep_on_screen(format!("m-{first}"));
                s.load_thumbs();
            },
        );
    }

    pub fn toggle_tool_output(&mut self, call: &str) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing_mut() else {
            return;
        };
        if chat.tool_outputs.remove(call).is_some() {
            return;
        }
        chat.tool_outputs.insert(call.to_owned(), None);
        self.fetch_tool_output(nav, call.to_owned());
    }

    fn fetch_tool_output(&mut self, nav: u64, call: String) {
        let Some(chat) = self.existing_at(nav) else {
            return;
        };
        let id = chat.id.clone();
        self.api.call(
            "tool_output",
            json!({"chat": id, "call": call}),
            move |s, result: Result<crate::data::ToolOutput, String>, _| {
                if let Some(chat) = s.existing_at(nav)
                    && let Some(output) = chat.tool_outputs.get_mut(&call)
                {
                    *output = Some(result);
                }
            },
        );
    }

    pub fn copy_message(&mut self, id: &str, tx: &mut Tx) {
        let Some(chat) = self.chat_mut().and_then(|p| p.existing_mut()) else {
            return;
        };
        let Some(answer) = chat
            .messages()
            .find(|m| m.id == id)
            .and_then(|m| match m.role {
                crate::data::Role::User => Some(m.text()),
                _ => m.answer(),
            })
        else {
            return;
        };
        self.copy_text(id, &answer, tx);
    }

    pub fn copy_text(&mut self, key: &str, text: &str, tx: &mut Tx) {
        tx.copy(text.to_owned());
        if let Some(chat) = self.chat_mut().and_then(|p| p.existing_mut()) {
            chat.copied = Some((key.to_owned(), Instant::now()));
        }
        self.sync_tick();
    }

    pub fn toggle_code_wrap(&mut self, key: &str) {
        if let Some(chat) = self.chat_mut().and_then(|p| p.existing_mut())
            && !chat.wrapped_code.remove(key)
        {
            chat.wrapped_code.insert(key.to_owned());
        }
    }

    pub fn load_info(&mut self) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let (args, project) = match &mut page.target {
            Target::Existing(chat) => {
                chat.info_at = Some(Instant::now());
                (json!({ "chat": chat.id }), None)
            }
            Target::New(draft) => (
                draft
                    .project
                    .as_ref()
                    .map_or_else(|| json!({}), |p| json!({ "project": p })),
                Some(draft.project.clone()),
            ),
        };
        self.api.call(
            "composer_info",
            args,
            move |s, result: Result<ComposerInfo, String>, _| {
                let Some(page) = s.page_at(nav) else { return };
                if let Some(project) = &project
                    && page.draft().is_none_or(|d| &d.project != project)
                {
                    return;
                }
                match result {
                    Ok(info) => {
                        page.info = Some(info);
                        s.load_catalog();
                    }
                    Err(e) => eprintln!("composer_info: {e}"),
                }
            },
        );
    }

    pub fn send(&mut self, tx: &mut Tx) {
        if self.chat().is_some_and(|page| page.queue_edit.is_some()) {
            self.save_queued(tx);
            return;
        }
        let Some(page) = self.chat_mut() else { return };
        let typed = page.composer.text.trim().to_owned();
        let creating = page.draft().is_some_and(|d| d.creating);
        if typed.is_empty() && page.attached.is_empty() || creating || !page.can_send() {
            return;
        }
        let images = match attach::ready(page) {
            Ok(images) => images,
            Err(why) => return self.fail(why, ""),
        };
        let nav = page.nav;
        let (payloads, thumbs): (Vec<_>, Vec<_>) = images.into_iter().unzip();
        let Target::Existing(chat) = &mut page.target else {
            self.create(typed, payloads);
            return;
        };
        let id = chat.id.clone();
        let at = chrono::Utc::now().timestamp_millis();
        chat.top_message = Some(at);
        chat.pending.push(Pending {
            text: typed.clone(),
            images: thumbs,
            at,
        });
        if matches!(page.voice, Voice::Listening(_)) {
            page.voice = Voice::Idle;
        }
        page.composer.clear();
        let attached = std::mem::take(&mut page.attached);
        tx.scroll_key_to("sent-turn", 10.0);
        let mut args = json!({ "chat": id, "text": typed });
        if !payloads.is_empty() {
            args["images"] = json!(payloads);
        }
        self.sync_tick();
        self.api.call(
            "send_message",
            args,
            move |s, result: Result<Sent, String>, _| {
                let Some(page) = s.page_at(nav) else {
                    if let Err(e) = result {
                        s.fail("Not sent", e);
                    }
                    return;
                };
                // A command pi handled itself never shows as a message.
                let gone = result.is_err()
                    || result
                        .as_ref()
                        .is_ok_and(|sent| matches!(sent.delivery.as_str(), "handled" | "queued"));
                if let Some(chat) = page.existing_mut() {
                    if gone && chat.top_message == Some(at) {
                        chat.top_message = None;
                    }
                    chat.pending.retain_mut(|p| {
                        if p.text == typed {
                            if gone {
                                return false;
                            }
                            if let Ok(sent) = &result {
                                p.text.clone_from(&sent.text);
                            }
                        }
                        true
                    });
                    let written = chat.page.take();
                    chat.confirm_pending(written.as_deref().unwrap_or_default(), None);
                    chat.page = written;
                }
                let Err(e) = result else { return };
                if page.composer.text.trim().is_empty() && page.attached.is_empty() {
                    page.composer = TextEdit::new(typed);
                    page.composer.set_caret(page.composer.text.len());
                    page.attached = attached;
                }
                s.fail("Not sent", e);
            },
        );
    }

    fn chat_action(&mut self, action: ChatAction) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing() else { return };
        let (tool, failed) = match &action {
            ChatAction::Stop => ("interrupt_chat", "Couldn't stop"),
            ChatAction::Respond(_) => ("respond_to_input", "Not sent"),
        };
        let args = match action {
            ChatAction::Stop => json!({ "chat": chat.id }),
            ChatAction::Respond(answer) => {
                let id = chat.question.as_ref().map(|q| q.dialog.id.clone());
                match answer {
                    Answer::Value(value) => json!({ "chat": chat.id, "id": id, "value": value }),
                    Answer::Confirmed(yes) => {
                        json!({ "chat": chat.id, "id": id, "confirmed": yes })
                    }
                }
            }
        };
        self.api.call(
            tool,
            args,
            move |s, result: Result<Value, String>, _| match result {
                Ok(_) => {}
                Err(e) => {
                    if let Some(q) = s.existing_at(nav).and_then(|c| c.question.as_mut()) {
                        q.sending = false;
                    }
                    s.fail(failed, e);
                }
            },
        );
    }

    pub fn stop(&mut self) {
        self.chat_action(ChatAction::Stop);
    }

    pub fn dictate(&mut self) {
        let Some(page) = self.chat_mut() else { return };
        page.composer.focused = true;
        match page.voice {
            Voice::Authorizing | Voice::Polishing => return,
            Voice::Listening(_) => return self.stop_dictating(),
            Voice::Idle => page.voice = Voice::Authorizing,
        }
        let nav = page.nav;
        let app = self.api.app().clone();
        dictation::authorize(move |allowed| {
            app.update(move |s, _| match allowed {
                Ok(()) => s.listen(nav),
                Err(why) => {
                    if let Some(page) = s.page_at(nav) {
                        page.voice = Voice::Idle;
                    }
                    s.fail("Can't dictate", why);
                }
            });
        });
    }

    pub fn stop_dictating(&mut self) {
        if let Some(d) = self.chat_mut().and_then(|p| p.voice.listening()) {
            d.session.stop();
        }
    }

    fn listen(&mut self, nav: u64) {
        let app = self.api.app().clone();
        let Some(page) = self.page_at(nav) else {
            return;
        };
        if !matches!(page.voice, Voice::Authorizing) {
            return;
        }
        let id = page.next_dictation;
        page.next_dictation += 1;
        let heard = move |heard: Heard| app.update(move |s, _| s.heard(nav, id, heard));
        match Listening::start(heard) {
            Ok(session) => {
                page.voice = Voice::Listening(Dictating {
                    id,
                    session,
                    started: Instant::now(),
                    heard: String::new(),
                    levels: VecDeque::new(),
                });
            }
            Err(why) => {
                page.voice = Voice::Idle;
                self.fail("Couldn't start dictation", why);
            }
        }
    }

    fn heard(&mut self, nav: u64, id: u64, heard: Heard) {
        let Some(page) = self.page_at(nav) else {
            return;
        };
        let Voice::Listening(d) = &mut page.voice else {
            return;
        };
        if d.id != id {
            return;
        }
        match heard {
            Heard::Words(words) => d.heard = words,
            Heard::Level(level) => {
                if d.levels.len() == LEVELS_KEPT {
                    d.levels.pop_front();
                }
                d.levels.push_back(level);
            }
            Heard::Done => {
                let Voice::Listening(d) = std::mem::replace(&mut page.voice, Voice::Idle) else {
                    return;
                };
                if !d.heard.trim().is_empty() {
                    self.polish(nav, d.heard);
                }
            }
            Heard::Failed(why) => {
                let Voice::Listening(d) = std::mem::replace(&mut page.voice, Voice::Idle) else {
                    return;
                };
                if !d.heard.trim().is_empty() {
                    self.add_dictated(nav, &d.heard);
                }
                self.fail("Dictation stopped", why);
            }
        }
    }

    fn polish(&mut self, nav: u64, heard: String) {
        let Some(page) = self.page_at(nav) else {
            return;
        };
        page.voice = Voice::Polishing;
        self.api.call(
            "polish_dictation",
            json!({ "text": heard }),
            move |s, result: Result<Polished, String>, _| {
                let words = match result {
                    Ok(polished) => polished.text,
                    Err(e) => {
                        s.fail(
                            "Couldn't clean up dictation",
                            format!("{e}. The words are as heard."),
                        );
                        heard
                    }
                };
                if let Some(page) = s.page_at(nav) {
                    page.voice = Voice::Idle;
                }
                s.add_dictated(nav, &words);
            },
        );
    }

    fn add_dictated(&mut self, nav: u64, words: &str) {
        let Some(page) = self.page_at(nav) else {
            return;
        };
        let text = dictation::join(&page.composer.text, words);
        page.composer.replace_text(text);
        page.composer.focused = true;
    }

    pub fn respond(&mut self, answer: Answer) {
        let Some(form) = self
            .chat_mut()
            .and_then(ChatPage::existing_mut)
            .and_then(|c| c.question.as_mut())
            .filter(|q| !q.sending)
        else {
            return;
        };
        form.sending = true;
        self.chat_action(ChatAction::Respond(answer));
    }

    pub fn respond_typed(&mut self) {
        let typed = self
            .chat()
            .and_then(ChatPage::existing)
            .and_then(|c| c.question.as_ref())
            .map(|q| q.field.text.clone());
        if let Some(typed) = typed {
            self.respond(Answer::Value(typed));
        }
    }

    pub fn open_link(&mut self, url: &str, tx: &mut Tx) {
        // Code links and pasted images are paths; Rune opens web links only.
        if url.starts_with('/') {
            self.open_path(url.to_owned());
        } else {
            drop(tx.open_url(url));
        }
    }

    pub fn open_chat_file(&mut self, path: &str) {
        let Some(chat) = self
            .chat()
            .and_then(ChatPage::existing)
            .and_then(|c| c.chat.as_ref())
        else {
            return;
        };
        let path = crate::paths::expand_home(&chat.cwd).join(crate::paths::expand_home(path));
        self.open_path(path.to_string_lossy().into_owned());
    }

    pub fn open_path(&mut self, path: String) {
        self.api.run(
            move |_| async move {
                tokio::process::Command::new("open")
                    .arg(path)
                    .output()
                    .await
            },
            |s, out, _| match out {
                Ok(out) if out.status.success() => {}
                Ok(out) => s.fail(
                    "Couldn't open the file",
                    String::from_utf8_lossy(&out.stderr).trim(),
                ),
                Err(e) => s.fail("Couldn't open the file", e.to_string()),
            },
        );
    }

    pub fn load_skills(&mut self) {
        let Some(page) = self.chat_mut() else { return };
        let args = match &page.target {
            Target::Existing(chat) => json!({ "chat": chat.id }),
            Target::New(d) => json!({ "project": d.project }),
        };
        let key = args.to_string();
        if page.skills.key == key {
            return;
        }
        page.skills.key.clone_from(&key);
        page.skills.list = SkillList::Loading;
        let nav = page.nav;
        self.api.call(
            "list_skills",
            args,
            move |s, result: Result<Skills, String>, _| {
                let Some(skills) = s
                    .page_at(nav)
                    .map(|p| &mut p.skills)
                    .filter(|m| m.key == key)
                else {
                    return;
                };
                skills.list = match result {
                    Ok(got) => SkillList::Ready(got.skills),
                    Err(e) => {
                        skills.key.clear();
                        SkillList::Failed(e)
                    }
                };
            },
        );
    }

    pub(super) fn reload_skills(&mut self) {
        if let Some(page) = self.chat_mut() {
            page.skills = SkillMenu::default();
        }
        self.load_skills();
    }
}
