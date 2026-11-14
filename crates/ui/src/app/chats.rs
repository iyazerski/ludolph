//! What a chat's menus do to it: open it in the other window, rename it, or
//! delete it after asking.

use rune::{PointerPress, PopupItem, PopupMenu, TextEdit, Tx};
use serde_json::{Value, json};

use super::{State, Window};

/// The dialog a chat's menu opened.
pub enum ChatDialog {
    /// The new title, as it's typed.
    Rename { chat: String, title: Box<TextEdit> },
    /// Whether to delete the chat, shown by its title.
    Delete { chat: String, title: String },
}

impl State {
    /// Open chat `id`'s context menu at `at`, as a right-click in a list asks.
    pub fn chat_menu(&mut self, id: String, at: PointerPress, tx: &mut Tx) {
        let other = match self.window {
            Window::Main => "Open in Quick Window",
            Window::Quick => "Open in Main Window",
        };
        let menu = PopupMenu::new()
            .item(PopupItem::new("peer", other))
            .separator()
            .item(PopupItem::new("rename", "Rename…"))
            .item(PopupItem::new("delete", "Delete…"));
        tx.popup_menu(menu, at, move |s: &mut State, picked, _| match picked {
            Some("peer") => s.open_in_peer(id),
            Some("rename") => s.ask_rename(id),
            Some("delete") => s.ask_delete(id),
            _ => {}
        });
    }

    /// Ask for chat `id`'s new title, starting from its current one.
    pub fn ask_rename(&mut self, id: String) {
        let mut title = TextEdit::new(self.title_of(&id));
        title.set_selection(0, title.text.len());
        title.focused = true;
        self.chat_dialog = Some(ChatDialog::Rename {
            chat: id,
            title: Box::new(title),
        });
    }

    /// Ask whether to delete chat `id`.
    pub fn ask_delete(&mut self, id: String) {
        let title = self.title_of(&id);
        self.chat_dialog = Some(ChatDialog::Delete { chat: id, title });
    }

    pub fn close_chat_dialog(&mut self) {
        self.chat_dialog = None;
    }

    /// The rename dialog's title field, or a scratch one once it's closed.
    pub fn rename_field(&mut self) -> &mut TextEdit {
        match &mut self.chat_dialog {
            Some(ChatDialog::Rename { title, .. }) => title,
            _ => &mut self.scratch_title,
        }
    }

    /// Rename the chat to what the dialog holds; the list's feed shows it.
    pub fn rename_chat(&mut self) {
        let Some(ChatDialog::Rename { chat, title }) = &self.chat_dialog else {
            return;
        };
        let title = title.text.trim().to_owned();
        if title.is_empty() {
            return;
        }
        let args = json!({ "chat": chat, "title": title });
        self.chat_dialog = None;
        self.api.call(
            "rename_chat",
            args,
            |s, result: Result<Value, String>, _| {
                if let Err(e) = result {
                    s.fail("Couldn't rename", e);
                }
            },
        );
    }

    /// Delete the chat the dialog asked about, and leave it in both windows.
    pub fn delete_chat(&mut self) {
        let Some(ChatDialog::Delete { chat, .. }) = self.chat_dialog.take() else {
            return;
        };
        let args = json!({ "chat": chat });
        self.api.call(
            "delete_chat",
            args,
            move |s, result: Result<Value, String>, _| match result {
                Ok(_) => {
                    s.notify("Chat deleted");
                    s.chat_gone(&chat);
                    s.api.peer().update(move |s, _| s.chat_gone(&chat));
                }
                Err(e) => s.fail("Couldn't delete", e),
            },
        );
    }

    /// Leave chat `id`'s page, if it shows, now the chat is gone.
    fn chat_gone(&mut self, id: &str) {
        if self.shows_chat(id) {
            self.leave_chat();
        }
    }

    /// Chat `id`'s title as the list shows it, or a page's if the list
    /// hasn't loaded it.
    fn title_of(&self, id: &str) -> String {
        let listed = self.list.chats.iter().flatten().find(|c| c.id == id);
        let shown = self
            .chat()
            .and_then(|p| p.existing())
            .filter(|c| c.id == id);
        listed
            .map(|c| c.title().to_owned())
            .or_else(|| {
                shown
                    .and_then(|c| c.chat.as_ref())
                    .map(|c| c.title().to_owned())
            })
            .unwrap_or_default()
    }
}
