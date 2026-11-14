//! The new-chat folder popover, anchored to its composer chip.

use rune::{El, Key, KeyEvent, TextEdit, Tx, col, icon, row, text};
use rune_kit::{Kit, Placement, Popover, TextInput};

use super::icons;
use super::widgets::tile;
use crate::app::{ChatPage, Draft, State};
use crate::data::Project;

const NEW: &str = "new-project";
const NONE: &str = "no-project";

/// Show searchable folders with actions that remain visible while filtering.
pub fn picker(page: &ChatPage, k: &Kit, layer: &mut Vec<El>) -> El {
    let Some(draft) = page.draft() else {
        return rune::el();
    };
    let selected = draft
        .projects
        .iter()
        .find(|p| Some(&p.id) == draft.project.as_ref());
    let trigger = k
        .interactive(
            row()
                .gap(5.0)
                .px(6.0)
                .py(3.0)
                .radius(7.0)
                .center_y()
                .child(tile(k, selected, 13.0))
                .child(
                    text(selected.map_or("No project", |p| p.name.as_str()))
                        .font(k.ty.small)
                        .color(k.c.text_muted)
                        .truncate(),
                ),
        )
        .key("project-chip")
        .label("Project")
        .on_press(|s: &mut State| {
            if let Some(draft) = s.chat_mut().and_then(|p| p.draft_mut()) {
                let menu = &mut draft.project_menu;
                menu.open = !menu.open;
                menu.search.clear();
                menu.search.focused = menu.open;
                menu.active = 0;
            }
        });
    let popover = Popover::new("project-picker")
        .trigger(trigger)
        .label("Project")
        .placement(Placement::TopStart)
        .padding(0.0)
        .open(draft.project_menu.open)
        .on_close(close);
    if !draft.project_menu.open {
        return popover.build(k, layer);
    }
    let projects = matching(draft);
    let active = draft.project_menu.active.min(projects.len() + 1);
    let rows = projects.iter().enumerate().map(|(i, p)| {
        let id = p.id.clone();
        k.interactive(
            row()
                .w_full()
                .h(30.0)
                .px(8.0)
                .gap(8.0)
                .center_y()
                .radius(6.0)
                .fill(if active == i {
                    k.c.hover
                } else {
                    rune::Color::TRANSPARENT
                })
                .child(icon(&icons::FOLDER, 16.0).color(k.c.icon_muted))
                .child(
                    text(&p.name)
                        .font(k.ty.ui)
                        .color(k.c.text_base)
                        .grow(1.0)
                        .min_w(0.0)
                        .truncate(),
                )
                .child(icon(&icons::CHECK, 13.0).color(k.c.icon_base).opacity(
                    if Some(&p.id) == draft.project.as_ref() {
                        1.0
                    } else {
                        0.0
                    },
                )),
        )
        .key(format!("project-options-option-{}", p.id))
        .role("option")
        .label(&p.name)
        .focus_selected(active == i)
        .position_in_set(i + 1, projects.len())
        .on_hover(move |s: &mut State| {
            if let Some(draft) = s.chat_mut().and_then(|p| p.draft_mut()) {
                draft.project_menu.active = i;
            }
        })
        .on_press(move |s: &mut State| s.pick_project(Some(id.clone())))
    });
    let search = TextInput::new(&draft.project_menu.search)
        .plain()
        .leading_icon(k.glyph.search)
        .width(None)
        .height(36.0)
        .placeholder("Search projects")
        .label("Search projects")
        .on_edit(search_field)
        .on_change(|s: &mut State, _| {
            if let Some(draft) = s.chat_mut().and_then(|p| p.draft_mut()) {
                draft.project_menu.active = 0;
            }
        })
        .on_key(keys)
        .build(k)
        .key("project-search");
    let list = if projects.is_empty() {
        col().w_full().px(8.0).py(10.0).child(
            text("No matching projects")
                .font(k.ty.ui)
                .color(k.c.text_muted),
        )
    } else {
        col()
            .key("project-viewport")
            .w_full()
            .h((projects.len() as f32 * 30.0).min(224.0))
            .min_h(30.0)
            .shrink()
            .role("listbox")
            .label("Projects")
            .scroll(rune::ScrollSpec {
                stick_bottom: false,
            })
            .child(
                col()
                    .w_full()
                    .growth(rune::GrowthSpec::default())
                    .virtualize(30.0)
                    .reveal_selected(true)
                    .children(rows),
            )
    }
    .preserve_focus_on_press(true);
    let body = col()
        .w(300.0)
        .shrink()
        .min_h(0.0)
        .child(row().w_full().px(6.0).child(search))
        .child(divider(k))
        .child(col().w_full().padding(4.0).shrink().min_h(0.0).child(list))
        .child(divider(k))
        .child(
            col()
                .w_full()
                .padding(4.0)
                .gap(2.0)
                .child(action(
                    k,
                    NEW,
                    "New project",
                    &icons::PLUS,
                    active == projects.len(),
                ))
                .child(action(
                    k,
                    NONE,
                    "Don't work in a project",
                    &icons::CLOSE,
                    active == projects.len() + 1,
                )),
        );
    popover.child(body).build(k, layer)
}

/// Match names and full paths without changing the saved project order.
fn matching(draft: &Draft) -> Vec<&Project> {
    let query = draft.project_menu.search.text.trim().to_lowercase();
    draft
        .projects
        .iter()
        .filter(|p| {
            p.name.to_lowercase().contains(&query) || p.path.to_lowercase().contains(&query)
        })
        .collect()
}

fn divider(k: &Kit) -> El {
    rune::el()
        .w_full()
        .h(1.0)
        .fill(k.c.border_weak)
        .a11y_hidden()
}

fn action(
    k: &Kit,
    id: &'static str,
    label: &'static str,
    glyph: &'static rune::Icon,
    active: bool,
) -> El {
    k.interactive(
        row()
            .w_full()
            .gap(8.0)
            .px(8.0)
            .py(7.0)
            .radius(6.0)
            .center_y()
            .fill(if active {
                k.c.text_base.alpha(0.08)
            } else {
                rune::Color::TRANSPARENT
            })
            .child(icon(glyph, 16.0).color(k.c.icon_muted))
            .child(text(label).font(k.ty.ui).color(k.c.text_base)),
    )
    .key(id)
    .label(label)
    .on_press_tx(move |s: &mut State, tx| choose(s, id, tx))
}

fn search_field(s: &mut State) -> &mut TextEdit {
    &mut s
        .chat_mut()
        .expect("the project search is on a chat")
        .draft_mut()
        .expect("only drafts choose projects")
        .project_menu
        .search
}

fn close(s: &mut State) {
    if let Some(draft) = s.chat_mut().and_then(|p| p.draft_mut()) {
        draft.project_menu.open = false;
        draft.project_menu.search.focused = false;
    }
}

fn choose(s: &mut State, id: &str, tx: &mut Tx) {
    match id {
        NEW => s.new_project(tx),
        NONE => s.pick_project(None),
        id => s.pick_project(Some(id.to_owned())),
    }
}

fn keys(s: &mut State, event: &KeyEvent, tx: &mut Tx) -> bool {
    let Some(draft) = s.chat_mut().and_then(|p| p.draft_mut()) else {
        return false;
    };
    let mut ids: Vec<String> = matching(draft).iter().map(|p| p.id.clone()).collect();
    ids.extend([NEW.to_owned(), NONE.to_owned()]);
    let active = draft.project_menu.active.min(ids.len() - 1);
    match event.key {
        Key::Escape => close(s),
        Key::Down | Key::Up => {
            let next = if event.key == Key::Down {
                (active + 1) % ids.len()
            } else {
                (active + ids.len() - 1) % ids.len()
            };
            draft.project_menu.active = next;
            tx.scroll_into_view(format!("project-options-option-{}", ids[next]));
        }
        Key::Enter if !event.mods.shift => choose(s, &ids[active], tx),
        _ => return false,
    }
    true
}
