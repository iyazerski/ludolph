//! The dialogs a chat's menus open: its new title, and whether to delete it.

use rune::{El, Tx, row};
use rune_kit::{Button, ButtonVariant, Dialog, Kit, TextInput};

use crate::app::{ChatDialog, State};

/// The open chat dialog, if any, onto `layer`.
pub fn chat_dialog(s: &State, k: &Kit, layer: &mut Vec<El>) {
    let Some(dialog) = &s.chat_dialog else {
        return;
    };
    let dialog = match dialog {
        ChatDialog::Rename { title, .. } => {
            let blank = title.text.trim().is_empty();
            let field = TextInput::new(title)
                .width(None)
                .placeholder("Title")
                .on_edit(State::rename_field)
                .on_submit(|s: &mut State, _: &mut Tx| s.rename_chat())
                .build(k);
            Dialog::new("rename-chat", "Rename chat")
                .child(rune::col().w_full().px(16.0).child(field))
                .child(buttons(
                    k,
                    Button::new("Rename")
                        .variant(ButtonVariant::Neutral)
                        .disabled(blank)
                        .on_press(State::rename_chat)
                        .build(k),
                ))
        }
        ChatDialog::Delete { title, .. } => {
            let what = if title.is_empty() {
                "This chat".to_owned()
            } else {
                format!("“{title}”")
            };
            Dialog::new("delete-chat", "Delete this chat?")
                .description(format!(
                    "{what} and its pi session are deleted for good. If its agent is working, it stops."
                ))
                .child(buttons(
                    k,
                    Button::new("Delete")
                        .variant(ButtonVariant::Danger)
                        .on_press(State::delete_chat)
                        .build(k),
                ))
        }
    };
    dialog
        .open(true)
        .fit()
        .width(400.0)
        .on_close(State::close_chat_dialog)
        .build(k, layer);
}

/// Cancel, then the dialog's action, at its bottom right.
fn buttons(k: &Kit, action: El) -> El {
    row()
        .w_full()
        .padding(16.0)
        .gap(8.0)
        .justify_end()
        .child(
            Button::new("Cancel")
                .variant(ButtonVariant::Outline)
                .on_press(State::close_chat_dialog)
                .build(k),
        )
        .child(action)
}
