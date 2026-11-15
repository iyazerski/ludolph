use std::time::{Duration, Instant};

use serde_json::json;

use super::{ChatPage, State, Window};
use crate::data::ChatDetails;

const DETAILS_EVERY: Duration = Duration::from_secs(8);

impl State {
    pub fn toggle_details(&mut self) {
        if self.window == Window::Quick {
            if let Some(chat) = self.chat_mut().and_then(ChatPage::existing_mut) {
                chat.details_open = true;
                chat.details_at = None;
            }
            self.load_details();
            self.sync_tick();
            return;
        }
        let shown = self.prefs.borrow().show_details;
        self.prefs.borrow_mut().show_details = !shown;
        if !shown && let Some(chat) = self.chat_mut().and_then(|page| page.existing_mut()) {
            chat.details_at = None;
        }
        self.save_prefs();
        self.api.peer().update(|s, _| {
            s.load_details();
            s.sync_tick();
        });
        self.load_details();
        self.sync_tick();
    }

    pub(super) fn details_visible(&self) -> bool {
        self.chat()
            .and_then(ChatPage::existing)
            .is_some_and(|chat| {
                chat.changes.is_none()
                    && match self.window {
                        Window::Main => self.prefs.borrow().show_details,
                        Window::Quick => chat.details_open,
                    }
            })
    }

    pub(super) fn load_details(&mut self) {
        if !self.shown || !self.engine_up || !self.details_visible() {
            return;
        }
        let Some(page) = self.chat_mut() else { return };
        let nav = page.nav;
        let Some(chat) = page.existing_mut() else {
            return;
        };
        if chat.changes.is_some()
            || chat.details_loading
            || chat
                .details_at
                .is_some_and(|at| at.elapsed() < DETAILS_EVERY)
        {
            return;
        }
        chat.details_loading = true;
        let id = chat.id.clone();
        self.api.call(
            "chat_details",
            json!({"chat": id}),
            move |s, result: Result<ChatDetails, String>, _| {
                if let Some(chat) = s.existing_at(nav) {
                    chat.details_loading = false;
                    chat.details_at = Some(Instant::now());
                    chat.details = Some(result);
                }
            },
        );
    }
}
