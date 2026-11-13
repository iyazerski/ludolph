//! What a chat's run changed: the button under the transcript, and the page
//! with its unified diff that opens in place of the chat.

use rune::{El, Span, TextStyle, col, icon, rich, row, text};
use rune_kit::diff::DiffView;
use rune_kit::{IconButton, IconButtonVariant, Kit};

use super::icons;
use super::widgets::{busy, head, head_titles, hide_button};
use crate::app::{Changes, Existing, State, Window};
use crate::data::TurnChanges;

/// "3 files changed +12 −4", pressed to open the diff.
pub fn button(k: &Kit, counts: &TurnChanges) -> El {
    let small = TextStyle {
        size: 12.0,
        ..k.ty.ui
    };
    let files = match counts.files {
        1 => "1 file changed".to_owned(),
        n => format!("{n} files changed"),
    };
    let stats = format!("+{} −{}", counts.additions, counts.deletions);
    let plus = stats.find(' ').unwrap_or(stats.len());
    let spans = vec![
        Span::new(0, plus).color(k.c.success.fg),
        Span::new(plus + 1, stats.len()).color(k.c.danger.fg),
    ];
    let turn = counts.turn.clone();
    let face = row()
        .key("turn-changes")
        .gap(8.0)
        .px(10.0)
        .h(30.0)
        .center_y()
        .radius(8.0)
        .child(icon(&icons::DIFF, 14.0).color(k.c.icon_base))
        .child(text(files).font(small).color(k.c.text_muted))
        .child(rich(stats, spans).font(TextStyle {
            size: 11.0,
            ..k.ty.mono
        }))
        .child(
            icon(&icons::CHEV_DOWN, 14.0)
                .color(k.c.icon_faint)
                .rotate(-std::f32::consts::FRAC_PI_2),
        );
    row().w_full().child(
        k.interactive(face)
            .label("Show the changes of this turn")
            .on_press(move |s: &mut State| s.open_changes(&turn)),
    )
}

/// The diff page: the back button returns to the chat.
pub fn page(s: &State, chat: &Existing, changes: &Changes, k: &Kit) -> El {
    // The prompt that started the run names it; the chat's title if it's not shown.
    let prompt = chat
        .messages()
        .find(|m| m.created_at.to_string() == chat.changes_of)
        .map(|m| m.text().split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty());
    let title = prompt.unwrap_or_else(|| {
        let title = chat.chat.as_ref().map(|c| c.title()).unwrap_or_default();
        title.to_owned()
    });
    let back = IconButton::new(&icons::BACK)
        .variant(IconButtonVariant::Ghost)
        .label("Back")
        .help("Back to the chat (⌘[)")
        .on_press(State::back)
        .build(k);
    let titles = head_titles()
        .gap(1.0)
        .child(
            text("Turn changes")
                .font(k.ty.ui_medium)
                .color(k.c.text_base)
                .truncate(),
        )
        .child(
            text(title)
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .truncate(),
        );
    let mut bar = head("changes-head").child(back).child(titles);
    if s.window == Window::Quick {
        bar = bar.child(hide_button(k));
    }
    let body = match changes {
        Changes::Loading => note().child(busy(18.0, k)),
        Changes::Failed(why) => note().child(
            text(format!("Couldn't load the changes: {why}"))
                .font(k.ty.ui)
                .color(k.c.text_muted)
                .wrap(),
        ),
        Changes::Ready { diff, .. } if diff.files().is_empty() => note().child(
            text("No changes this turn")
                .font(k.ty.ui)
                .color(k.c.text_muted),
        ),
        Changes::Ready { truncated, .. } => {
            let mut out = col().w_full().grow(1.0).min_h(0.0);
            if *truncated {
                out = out.child(
                    text("The diff is too large to show whole.")
                        .font(k.ty.small)
                        .color(k.c.text_faint)
                        .px(14.0)
                        .py(6.0),
                );
            }
            out.child(
                DiffView::new(State::diff, State::diff_mut)
                    .key("turn-diff")
                    .build(s, k),
            )
        }
    };
    col().w_full().h_full().child(bar).child(body)
}

fn note() -> El {
    col()
        .w_full()
        .grow(1.0)
        .px(24.0)
        .justify_center()
        .items_center()
}
