//! Waiting messages above the composer, with edit, delete and steer controls.

use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, IconButton, IconButtonVariant, Kit};

use super::icons;
use crate::app::{ChatPage, State};

/// Show waiting prompts without putting them in the transcript before they are sent.
pub fn view(page: &ChatPage, k: &Kit) -> Option<El> {
    let chat = page.existing()?;
    if chat.queued.is_empty() {
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
        let steer_id = edit_id.clone();
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
                        .on_press_tx(move |s: &mut State, tx| s.edit_queued(&edit_id, tx))
                        .build(k),
                )
                .child(
                    IconButton::new(&icons::DELETE)
                        .variant(IconButtonVariant::GhostMuted)
                        .label("Delete queued message")
                        .on_press(move |s: &mut State| s.delete_queued(&delete_id))
                        .build(k),
                )
                .child(
                    IconButton::new(&icons::SEND)
                        .variant(IconButtonVariant::GhostMuted)
                        .label("Steer queued message")
                        .help("Send now to the agent")
                        .on_press(move |s: &mut State| s.steer_queued(&steer_id))
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
    Some(
        body.key("waiting-messages")
            .max_h(190.0)
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            }),
    )
}
