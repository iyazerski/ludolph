use rune::{Drag, Phase, PointerPress, PopupItem, PopupMenu, TextEdit, Tx};
use serde_json::{Value, json};

use super::{State, Window};

pub fn project_drop_key(id: &str) -> String {
    format!("drop-project-{}-{id}", id.len())
}

pub enum ChatDialog {
    Rename { chat: String, title: Box<TextEdit> },
    Delete { chat: String, title: String },
}

impl State {
    pub fn chat_menu(&mut self, id: String, at: PointerPress, tx: &mut Tx) {
        let other = match self.window {
            Window::Main => "Open in Quick Window",
            Window::Quick => "Open in Main Window",
        };
        let menu = PopupMenu::new()
            .item(PopupItem::new("peer", other))
            .separator()
            .item(PopupItem::new("copy-id", "Copy Chat ID"))
            .item(PopupItem::new("rename", "Rename…"))
            .item(PopupItem::new("delete", "Delete…"));
        tx.popup_menu(menu, at, move |s: &mut State, picked, tx| match picked {
            Some("peer") => s.open_in_peer(id),
            Some("copy-id") => tx.copy(&id),
            Some("rename") => s.ask_rename(id),
            Some("delete") => s.ask_delete(id),
            _ => {}
        });
    }

    pub fn drag_chat(&mut self, id: &str, drag: &Drag, tx: &mut Tx) {
        if drag.cancelled() {
            self.list.drag_chat = None;
            self.list.drop_project = None;
            return;
        }
        if drag.phase == Phase::Down
            || self.list.drag_chat.is_none() && drag.dx.hypot(drag.dy) < 4.0
        {
            return;
        }
        self.list.drag_chat = Some(id.to_owned());
        let id = id.to_owned();
        let dropped = drag.phase == Phase::Up;
        let [x, y] = drag.window_point();
        tx.element_at(x, y, move |s: &mut State, picked, _| {
            let target = picked.and_then(|picked| {
                s.list
                    .chats
                    .iter()
                    .flatten()
                    .filter_map(|chat| chat.project.as_ref())
                    .find(|project| {
                        let key = project_drop_key(&project.id);
                        picked.key() == Some(key.as_str())
                            || picked.path().contains(&format!("/{key}/"))
                    })
                    .map(|project| project.id.clone())
            });
            let source = s.list.chats.iter().flatten().find(|chat| chat.id == id);
            let target = target.filter(|target| source.is_some_and(|chat| chat.cwd != *target));
            s.list.drop_project = target.clone();
            if dropped {
                s.list.drag_chat = None;
                s.list.drop_project = None;
                if let Some(project) = target {
                    s.move_chat(id, project);
                }
            }
        });
    }

    fn move_chat(&mut self, chat: String, project: String) {
        self.api.call(
            "move_chat",
            json!({ "chat": chat, "project": project }),
            |s, result: Result<Value, String>, _| {
                if let Err(e) = result {
                    s.fail("Couldn't move the chat", e);
                }
            },
        );
    }

    pub fn ask_rename(&mut self, id: String) {
        let mut title = TextEdit::new(self.title_of(&id));
        title.set_selection(0, title.text.len());
        title.focused = true;
        self.chat_dialog = Some(ChatDialog::Rename {
            chat: id,
            title: Box::new(title),
        });
    }

    pub fn ask_delete(&mut self, id: String) {
        let title = self.title_of(&id);
        self.chat_dialog = Some(ChatDialog::Delete { chat: id, title });
    }

    pub fn close_chat_dialog(&mut self) {
        self.chat_dialog = None;
    }

    pub fn rename_field(&mut self) -> &mut TextEdit {
        match &mut self.chat_dialog {
            Some(ChatDialog::Rename { title, .. }) => title,
            _ => &mut self.scratch_title,
        }
    }

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
                    s.chat_gone(&chat);
                    s.api.peer().update(move |s, _| s.chat_gone(&chat));
                }
                Err(e) => s.fail("Couldn't delete", e),
            },
        );
    }

    fn chat_gone(&mut self, id: &str) {
        if self.shows_chat(id) {
            self.leave_chat();
        }
    }

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
