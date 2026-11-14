//! Unsent messages above the composer, each with edit and delete controls.

use rune::{El, Tx, auto_grow, col, row, text, text_input};
use rune_kit::{Button, ButtonVariant, IconButton, IconButtonVariant, Kit};

use super::icons;
use crate::app::{ChatPage, State};

/// Show waiting prompts without putting them in the transcript before they are sent.
pub fn view(page: &ChatPage, k: &Kit) -> Option<El> {
    let chat = page.existing()?;
    if chat.queued.is_empty() && page.queue_edit.is_none() {
        return None;
    }
    let mut header = row()
        .w_full()
        .center_y()
        .gap(8.0)
        .child(
            text(format!("Queued · {}", chat.queued.len()))
                .font(k.ty.small_medium)
                .color(k.c.text_muted),
        )
        .child(rune::spacer());
    if !page.is_working() && !chat.queued.is_empty() {
        header = header.child(
            Button::new("Send next")
                .variant(ButtonVariant::Ghost)
                .on_press(State::resume_queue)
                .build(k),
        );
    }
    let mut body = col().w_full().gap(6.0).child(header);
    for message in &chat.queued {
        let edit_id = message.id.clone();
        let delete_id = edit_id.clone();
        let mut entry = col()
            .w_full()
            .gap(3.0)
            .px(8.0)
            .py(4.0)
            .radius(8.0)
            .fill(k.c.text_base.alpha(0.035));
        let label = if message.images > 0 {
            format!("{}  · {} image(s)", message.text, message.images)
        } else {
            message.text.clone()
        };
        entry = entry.child(
            row()
                .w_full()
                .center_y()
                .gap(6.0)
                .child(
                    text(label)
                        .font(k.ty.small)
                        .color(k.c.text_muted)
                        .truncate()
                        .min_w(0.0)
                        .grow(1.0),
                )
                .child(
                    IconButton::new(&icons::EDIT)
                        .variant(IconButtonVariant::GhostMuted)
                        .label("Edit queued message")
                        .on_press(move |s: &mut State| s.edit_queued(&edit_id))
                        .build(k),
                )
                .child(
                    IconButton::new(&icons::DELETE)
                        .variant(IconButtonVariant::GhostMuted)
                        .label("Delete queued message")
                        .on_press(move |s: &mut State| s.delete_queued(&delete_id))
                        .build(k),
                ),
        );
        if let Some(error) = &message.error {
            entry = entry.child(
                text(error.clone())
                    .font(k.ty.small)
                    .color(k.c.danger.fg)
                    .wrap(),
            );
        }
        body = body.child(entry.key(format!("queue-{}", message.id)));
    }
    if let Some(edit) = &page.queue_edit {
        let field = text_input(&edit.field, k.ty.ui)
            .key("queue-editor")
            .label("Edit queued message")
            .color(k.c.text_base)
            .caret(k.c.text_base, k.selection())
            .spellcheck(true)
            .on_edit(State::queued_field)
            .on_submit(State::save_queued);
        body = body.child(
            col().w_full().gap(6.0).child(auto_grow(field, 1, 4)).child(
                row()
                    .w_full()
                    .justify_end()
                    .gap(6.0)
                    .child(
                        Button::new("Cancel")
                            .variant(ButtonVariant::Ghost)
                            .disabled(edit.saving)
                            .on_press(|s: &mut State| {
                                if let Some(page) = s.chat_mut() {
                                    page.queue_edit = None;
                                }
                            })
                            .build(k),
                    )
                    .child(
                        Button::new("Save")
                            .disabled(edit.saving)
                            .on_press_tx(|s: &mut State, tx: &mut Tx| s.save_queued(tx))
                            .build(k),
                    ),
            ),
        );
    }
    Some(
        body.key("waiting-messages")
            .max_h(190.0)
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            }),
    )
}
