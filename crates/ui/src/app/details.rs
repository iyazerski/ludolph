//! Load the optional chat details card only while it is on screen.

use std::time::{Duration, Instant};

use serde_json::json;

use super::State;
use crate::data::ChatDetails;

/// External working-tree changes need no chat event; refresh at most this often.
const DETAILS_EVERY: Duration = Duration::from_secs(8);

impl State {
    /// Show or hide chat details in both windows, remembering the choice.
    pub fn toggle_details(&mut self) {
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

    /// Refresh visible details without polling pi or the network.
    pub(super) fn load_details(&mut self) {
        if !self.shown || !self.engine_up || !self.prefs.borrow().show_details {
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
