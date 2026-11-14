//! Chat details as a main-window card or a separate quick-window page.

use rune::{El, Link, ScrollSpec, Span, TextStyle, Tx, col, icon, rich, row, text};
use rune_kit::{IconButton, IconButtonVariant, Kit};

use super::icons;
use super::widgets::{busy, head, head_titles, hide_button};
use crate::app::{Existing, State, Window};
use crate::data::ChatDetails;

/// Open quick-window details or toggle the main window's card.
pub fn toggle(s: &State, k: &Kit) -> El {
    let shown = s.window == Window::Main && s.prefs.borrow().show_details;
    IconButton::new(&icons::INFO)
        .variant(if shown {
            IconButtonVariant::Neutral
        } else {
            IconButtonVariant::GhostMuted
        })
        .label(if shown {
            "Hide chat details"
        } else {
            "Show chat details"
        })
        .help(if shown {
            "Hide chat details"
        } else {
            "Show chat details"
        })
        .expanded(shown)
        .on_press(State::toggle_details)
        .build(k)
}

/// Show quick-window details without squeezing the transcript or composer.
pub fn page(chat: &Existing, k: &Kit) -> El {
    let back = IconButton::new(&icons::BACK)
        .variant(IconButtonVariant::Ghost)
        .label("Back")
        .help("Back to the chat (⌘[)")
        .on_press(State::back)
        .build(k);
    col()
        .w_full()
        .h_full()
        .child(
            head("details-head")
                .child(back)
                .child(
                    head_titles().child(
                        text("Chat details")
                            .font(k.ty.ui_medium)
                            .color(k.c.text_base),
                    ),
                )
                .child(hide_button(k)),
        )
        .child(
            col()
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .padding(14.0)
                .child(card(chat, k))
                .scroll(ScrollSpec {
                    stick_bottom: false,
                }),
        )
}

/// Keep the card compact; long PR lists scroll independently.
pub fn card(chat: &Existing, k: &Kit) -> El {
    let project = chat
        .chat
        .as_ref()
        .and_then(|chat| chat.project.as_ref())
        .map(|project| project.name.as_str())
        .unwrap_or("Chat details");
    let mut body = col().w_full().padding(14.0).gap(12.0).child(
        text(project.to_owned())
            .font(k.ty.small)
            .color(k.c.text_muted)
            .truncate(),
    );
    body = match &chat.details {
        None => body.child(busy(14.0, k)),
        Some(Err(error)) => body.child(
            text(error.clone())
                .font(k.ty.small)
                .color(k.c.danger.fg)
                .wrap(),
        ),
        Some(Ok(details)) => content(body, details, k),
    };
    body.key("chat-details")
        .radius(14.0)
        .fill(k.c.bg_base)
        .border(1.0, k.c.border_muted)
}

/// Render only recorded facts, without fetching PR status or source links.
fn content(mut body: El, details: &ChatDetails, k: &Kit) -> El {
    if let Some(counts) = &details.changes {
        let stats = format!("+{} −{}", counts.additions, counts.deletions);
        let split = stats.find(' ').unwrap_or(stats.len());
        let line = row()
            .w_full()
            .h(30.0)
            .center_y()
            .gap(8.0)
            .child(icon(&icons::DIFF, 16.0).color(k.c.icon_base))
            .child(text("Changes").font(k.ty.ui).color(k.c.text_base).grow(1.0))
            .child(
                rich(
                    stats.clone(),
                    vec![
                        Span::new(0, split).color(k.c.success.fg),
                        Span::new(split + 1, stats.len()).color(k.c.danger.fg),
                    ],
                )
                .font(TextStyle {
                    size: 11.0,
                    ..k.ty.mono
                }),
            );
        body = body.child(
            k.interactive(line.radius(6.0))
                .label(format!(
                    "Show current uncommitted changes in {} files",
                    counts.files
                ))
                .on_press(State::open_working_changes),
        );
    } else {
        let label = details
            .changes_error
            .as_deref()
            .unwrap_or("Not a Git repository");
        body = body.child(
            text(label.to_owned())
                .font(k.ty.small)
                .color(k.c.text_faint)
                .wrap(),
        );
    }
    body = body
        .child(rule(k))
        .child(text("Pull requests").font(k.ty.small).color(k.c.text_faint));
    if details.pull_requests.is_empty() {
        body = body.child(
            text("No PR links yet")
                .font(k.ty.small)
                .color(k.c.text_faint),
        );
    } else {
        let links = details.pull_requests.iter().map(|pr| {
            let label = pr.label.clone();
            row()
                .w_full()
                .gap(8.0)
                .center_y()
                .child(icon(&icons::FETCH, 14.0).color(k.c.icon_faint))
                .child(
                    rich(label.clone(), Vec::new())
                        .font(k.ty.small)
                        .color(k.c.text_base)
                        .truncate()
                        .min_w(0.0)
                        .link(Link::new(0..label.len(), pr.url.clone()))
                        .on_link(|s: &mut State, url: &str, tx: &mut Tx| s.open_link(url, tx)),
                )
        });
        body = body.child(
            col()
                .w_full()
                .max_h(150.0)
                .gap(8.0)
                .scroll(ScrollSpec {
                    stick_bottom: false,
                })
                .children(links),
        );
    }
    body.child(rule(k)).child(
        text(format!(
            "{} {} · {:.2}M · {}",
            details.compactions,
            if details.compactions == 1 {
                "compaction"
            } else {
                "compactions"
            },
            details.spent.tokens as f64 / 1_000_000.0,
            crate::labels::dollars(details.spent.cost)
        ))
        .font(TextStyle {
            size: 11.0,
            ..k.ty.ui
        })
        .color(k.c.text_muted)
        .wrap()
        .label("Session totals: compactions, tokens in millions, and cost in US dollars"),
    )
}

fn rule(k: &Kit) -> El {
    rune::el().w_full().h(1.0).fill(k.c.border_muted)
}
