//! What a chat's page does with the engine: follow the chat's feed, load
//! earlier messages, send, answer pi's dialogs, stop, archive, dictate, and
//! load what the composer shows.

use std::collections::{HashMap, VecDeque};
use std::time::Instant;

use rune::{TextEdit, Tx};
use rune_kit::diff::{self, DiffState};
use serde::Deserialize;
use serde_json::{Value, json};

use super::page::{Changes, Dictating, LEVELS_KEPT, SkillList, SkillMenu, Voice};
use super::{ChatPage, Pending, QuestionForm, State, Target, attach};
use crate::data::{
    ChatView, ComposerInfo, Diff, Frame, Message, MessageStatus, Polished, Role, Skills,
};
use crate::dictation::{self, Heard, Listening};
use crate::message::with_skill_refs;

/// How many more messages "Load earlier messages" adds.
const EARLIER: usize = 40;
/// How much earlier than its send a message's own timestamp may be.
const CLOCK_SLACK_MS: i64 = 2000;

/// A call on the open chat; the feed shows what it changes.
enum ChatAction {
    Stop,
    Archive,
    Respond(Answer),
}

/// An answer to pi's dialog.
pub enum Answer {
    /// A select's option, or the typed text.
    Value(String),
    Confirmed(bool),
}

/// `send_message`'s answer.
#[derive(Deserialize)]
struct Sent {
    delivery: String,
}

impl State {
    /// Show one frame of chat `nav`'s feed: its page, run and dialog.
    pub(super) fn frame(&mut self, nav: u64, record: Value, tx: &mut Tx) {
        let Some(chat) = self.existing_at(nav) else {
            return;
        };
        if let Some(why) = record.get("error") {
            // The engine ended the feed: the chat can't be read.
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
        chat.error = None;
        // A streaming reply grows every frame, and springing the page to each new
        // size keeps the window drawing at the display's rate for the whole reply.
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
        let queued = frame
            .run
            .iter()
            .flat_map(|r| r.steering.iter().chain(&r.follow_up));
        let queued: Vec<&str> = queued.map(|t| t.trim()).collect();
        chat.pending.retain(|p| {
            let arrived = page.iter().any(|m| {
                m.role == Role::User
                    && m.created_at >= p.at - CLOCK_SLACK_MS
                    && m.text().trim() == p.text
                    && m.images().count() == p.images.len()
            });
            !arrived && !(p.images.is_empty() && queued.contains(&p.text.as_str()))
        });
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
        chat.page = Some(page);
        chat.file_links.borrow_mut().clear();
        // A run that ends may have changed the model, the context or the branch.
        let settled =
            chat.chat.as_ref().is_some_and(|c| c.is_working()) && !frame.chat.is_working();
        let seen = self.prefs.borrow_mut().mark_seen(&frame.chat);
        if seen {
            self.save_prefs();
        }
        if let Some(chat) = self.existing_at(nav) {
            let due = settled && chat.info_due();
            chat.chat = Some(frame.chat);
            self.load_thumbs();
            if due {
                self.load_info();
            }
        }
        self.sync_tick();
    }

    /// Open the diff of the chat's run `turn` in place of its transcript.
    pub fn open_changes(&mut self, turn: &str) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing_mut() else {
            return;
        };
        chat.changes = Some(Changes::Loading);
        turn.clone_into(&mut chat.changes_of);
        let args = json!({ "chat": chat.id, "turn": turn });
        self.api.call(
            "turn_changes",
            args,
            move |s, result: Result<Diff, String>, _| {
                let Some(chat) = s.existing_at(nav).filter(|c| c.changes.is_some()) else {
                    return;
                };
                chat.changes = Some(match result {
                    Ok(got) => {
                        let mut diff = Box::new(DiffState::new(diff::parse(&got.patch)));
                        // The window is too narrow to scroll lines sideways.
                        diff.set_wrap(true);
                        Changes::Ready {
                            diff,
                            truncated: got.truncated,
                        }
                    }
                    Err(e) => Changes::Failed(e),
                });
            },
        );
    }

    /// Load the messages before the first one shown. They stay until the
    /// feed resets.
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
                // The first one shown may have gone, as after a /tree.
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

    /// Copy message `id`'s answer as Markdown, and mark its button copied.
    pub fn copy_answer(&mut self, id: &str, tx: &mut Tx) {
        let Some(chat) = self.chat_mut().and_then(|p| p.existing_mut()) else {
            return;
        };
        let Some(answer) = chat
            .messages()
            .find(|m| m.id == id)
            .and_then(Message::answer)
        else {
            return;
        };
        tx.copy(answer);
        chat.copied = Some((id.to_owned(), Instant::now()));
        self.sync_tick();
    }

    /// The composer's branch, context and model: the chat's, or a new chat's in its project.
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
                // A new chat's project may have changed while this loaded.
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

    /// Send the composer's text and images: to the chat, or as a new chat's
    /// first message.
    pub fn send(&mut self, tx: &mut Tx) {
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
        let text = with_skill_refs(&typed, page.skills.list.ready());
        let nav = page.nav;
        let payloads: Vec<attach::Payload> = images.iter().map(|(p, _)| p.clone()).collect();
        let Target::Existing(chat) = &mut page.target else {
            self.create(text, payloads);
            return;
        };
        let id = chat.id.clone();
        chat.pending.push(Pending {
            text: typed.clone(),
            images: images.iter().map(|(_, thumb)| thumb.clone()).collect(),
            at: chrono::Utc::now().timestamp_millis(),
        });
        if matches!(page.voice, Voice::Listening(_)) {
            page.voice = Voice::Idle;
        }
        page.composer.clear();
        page.attached.clear();
        tx.scroll_to_bottom("transcript");
        let mut args = json!({ "chat": id, "text": text });
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
                let failed = result.as_ref().err().cloned();
                // A command pi handled itself never shows as a message.
                let gone = failed.is_some()
                    || result.as_ref().is_ok_and(|sent| sent.delivery == "handled");
                if gone && let Some(chat) = page.existing_mut() {
                    chat.pending.retain(|p| p.text != typed);
                }
                let Some(e) = failed else { return };
                if page.composer.text.trim().is_empty() && page.attached.is_empty() {
                    page.composer = TextEdit::new(typed);
                    page.composer.set_caret(page.composer.text.len());
                    s.reattach(nav, images);
                }
                s.fail("Not sent", e);
            },
        );
    }

    fn chat_action(&mut self, action: ChatAction) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing() else { return };
        let archive = matches!(action, ChatAction::Archive);
        let (tool, failed) = match &action {
            ChatAction::Stop => ("interrupt_chat", "Couldn't stop"),
            ChatAction::Archive => ("archive_chat", "Couldn't archive"),
            ChatAction::Respond(_) => ("respond_to_input", "Not sent"),
        };
        let args = match action {
            ChatAction::Stop | ChatAction::Archive => json!({ "chat": chat.id }),
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
                Ok(_) if archive => {
                    s.notify("Chat archived");
                    if s.page_at(nav).is_some() {
                        s.leave_chat();
                    }
                }
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

    pub fn archive(&mut self) {
        self.chat_action(ChatAction::Archive);
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

    /// Stop listening; what was heard goes on to be cleaned up.
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

    /// Keep what dictation `id` heard, out of sight until it's cleaned up.
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
            // What was heard before the failure goes in as heard, rather than being lost.
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

    /// Have the model turn what dictation heard into a clean message, put
    /// after the composer's text. A failed cleanup puts the words as heard.
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

    /// Put dictated `words` after the composer's text on page `nav`.
    fn add_dictated(&mut self, nav: u64, words: &str) {
        let Some(page) = self.page_at(nav) else {
            return;
        };
        let text = dictation::join(&page.composer.text, words);
        page.composer.replace_text(text);
        page.composer.focused = true;
    }

    /// Answer pi's dialog.
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

    /// Answer an input or editor dialog with what's typed.
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

    /// Open a file or folder the transcript names in its default app.
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

    /// Load the skills for the `$` menu: the chat's, or a new chat's in its project.
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
                        // Forget the scope, so the next `$` tries again.
                        skills.key.clear();
                        SkillList::Failed(e)
                    }
                };
            },
        );
    }

    /// Drop the skills of a new chat's old project, and load its new ones.
    pub(super) fn reload_skills(&mut self) {
        if let Some(page) = self.chat_mut() {
            page.skills = SkillMenu::default();
        }
        self.load_skills();
    }
}
