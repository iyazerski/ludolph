//! Edit waiting prompts in the composer and send individual prompts as steering.

use rune::{TextEdit, Tx};
use serde_json::{Value, json};

use super::page::QueueEdit;
use super::{State, Voice};
use crate::message::with_skill_refs;

impl State {
    /// Edit a waiting prompt in the composer, saving its unsent draft until done.
    pub fn edit_queued(&mut self, id: &str, tx: &mut Tx) {
        let Some(page) = self.chat_mut() else { return };
        let Some(message) = page
            .existing()
            .and_then(|chat| chat.queued.iter().find(|m| m.id == id))
        else {
            return;
        };
        let (text, images) = (message.text.clone(), message.images);
        if page.queue_edit.as_ref().is_some_and(|edit| edit.saving) {
            return;
        }
        page.cancel_queue_edit();
        page.voice = Voice::Idle;
        let mut field = TextEdit::new(text);
        field.set_caret(field.text.len());
        field.focused = true;
        let draft = std::mem::replace(&mut page.composer, field);
        page.queue_edit = Some(QueueEdit {
            id: id.to_owned(),
            draft,
            attached: std::mem::take(&mut page.attached),
            images,
            saving: false,
        });
        tx.focus("composer", false);
    }

    /// Save the waiting prompt's text, keeping its images and place.
    pub fn save_queued(&mut self, _: &mut Tx) {
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing() else { return };
        let chat = chat.id.clone();
        let Some(edit) = page.queue_edit.as_mut().filter(|edit| !edit.saving) else {
            return;
        };
        let id = edit.id.clone();
        let text = with_skill_refs(page.composer.text.trim(), page.skills.list.ready());
        edit.saving = true;
        self.api.call(
            "edit_queued_message",
            json!({"chat": chat, "id": id, "text": text}),
            move |s, result: Result<Value, String>, _| {
                if let Some(page) = s.page_at(nav)
                    && let Some(edit) = page.queue_edit.as_mut().filter(|edit| edit.id == id)
                {
                    edit.saving = false;
                    if result.is_ok() {
                        page.cancel_queue_edit();
                    }
                }
                if let Err(error) = result {
                    s.fail("Couldn't edit the queued message", error);
                }
            },
        );
    }

    /// Delete just this waiting prompt, not other messages in the queue.
    pub fn delete_queued(&mut self, id: &str) {
        let Some(page) = self.chat() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing() else { return };
        let id = id.to_owned();
        self.api.call(
            "delete_queued_message",
            json!({"chat": chat.id, "id": id}),
            move |s, result: Result<Value, String>, _| {
                if result.is_ok()
                    && let Some(page) = s.page_at(nav)
                    && page.queue_edit.as_ref().is_some_and(|edit| edit.id == id)
                {
                    page.cancel_queue_edit();
                }
                if let Err(error) = result {
                    s.fail("Couldn't remove the queued message", error);
                }
            },
        );
    }

    /// Retry the oldest waiting prompt after a stopped or failed run.
    pub fn resume_queue(&mut self) {
        let Some(chat) = self.chat().and_then(|page| page.existing()) else {
            return;
        };
        self.api.call(
            "resume_queue",
            json!({"chat": chat.id}),
            |s, result: Result<Value, String>, _| {
                if let Err(error) = result {
                    s.fail("Couldn't send the queued message", error);
                }
            },
        );
    }

    /// Leave the queue edit and restore the composer's unsent draft.
    pub fn cancel_queue_edit(&mut self) {
        if let Some(page) = self.chat_mut() {
            page.cancel_queue_edit();
        }
    }

    /// Send one waiting prompt to pi now, bypassing the default queue behavior.
    pub fn steer_queued(&mut self, id: &str) {
        let Some(page) = self.chat() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing() else { return };
        let id = id.to_owned();
        self.api.call(
            "steer_queued_message",
            json!({"chat": chat.id, "id": id}),
            move |s, result: Result<Value, String>, _| {
                if result.is_ok()
                    && let Some(page) = s.page_at(nav)
                    && page.queue_edit.as_ref().is_some_and(|edit| edit.id == id)
                {
                    page.cancel_queue_edit();
                }
                if let Err(error) = result {
                    s.fail("Couldn't steer the queued message", error);
                }
            },
        );
    }
}
