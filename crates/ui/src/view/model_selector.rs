use std::rc::Rc;

use rune::{Color, Drag, El, Key, KeyEvent, Phase, Tx, col, el, icon, row, text};
use rune_kit::{
    ControlSize, IconButton, IconButtonVariant, Kit, Placement, TextInput, popover, scrim,
};

use super::icons;
use crate::app::{ModelSelectorState, ModelSelectorStep, State};
use crate::data::Model;
use crate::labels::{reasoning_name, short_model};

const POPOVER_WIDTH: f32 = 264.0;
const CARD_PAD: f32 = 10.0;
const CARD_RADIUS: f32 = 14.0;
const PILL_H: f32 = 26.0;
const PILL_INSET: f32 = 2.0;
const PILL_THUMB: f32 = PILL_H - 2.0 * PILL_INSET;
const PILL_DOT: f32 = 4.0;
const ROW_H: f32 = 26.0;
const LIST_MAX_H: f32 = 208.0;
const TRIGGER_H: f32 = 28.0;

type SelectorAccess = Rc<dyn Fn(&mut State) -> Option<&mut ModelSelectorState>>;
type ModelChoose = Rc<dyn Fn(&mut State, &str, &mut Tx)>;
type ReasoningChoose = Rc<dyn Fn(&mut State, &str)>;

#[derive(Clone, Debug)]
pub struct ModelOption {
    pub id: String,
    pub label: String,
    pub detail: String,
    pub mark_id: Option<String>,
    pub has_reasoning: bool,
}

impl ModelOption {
    pub fn new(id: impl Into<String>, label: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: String::new(),
            mark_id: None,
            has_reasoning: false,
        }
    }

    pub fn mark(mut self, id: Option<String>) -> Self {
        self.mark_id = id;
        self
    }

    pub fn has_reasoning(mut self, has: bool) -> Self {
        self.has_reasoning = has;
        self
    }
}

pub fn model_options<'a>(
    models: impl IntoIterator<Item = &'a Model>,
    allow_reasoning: bool,
) -> Vec<ModelOption> {
    models
        .into_iter()
        .map(|model| {
            let label = model
                .label
                .clone()
                .unwrap_or_else(|| short_model(&model.id).to_owned());
            let detail = model
                .id
                .split_once('/')
                .map_or("", |(provider, _)| provider)
                .to_owned();
            ModelOption {
                id: model.id.clone(),
                label,
                detail,
                mark_id: Some(model.id.clone()),
                has_reasoning: allow_reasoning && model.reasoning_levels.len() > 1,
            }
        })
        .collect()
}

/// Unified model and reasoning effort selector: one trigger pill that opens a compact card with
/// the model header and pill reasoning slider, or drills into a searchable model list.
pub struct ModelSelector<'a> {
    key: String,
    label: String,
    state: &'a ModelSelectorState,
    options: &'a [ModelOption],
    current_model: Option<&'a str>,
    mark_model: Option<&'a str>,
    placeholder: String,
    empty_message: String,
    reasoning_levels: &'a [String],
    current_reasoning: Option<&'a str>,
    placement: Placement,
    trigger_width: Option<f32>,
    popover_width: f32,
    disabled: bool,
    access: SelectorAccess,
    on_choose_model: Option<ModelChoose>,
    on_choose_reasoning: Option<ReasoningChoose>,
}

impl<'a> ModelSelector<'a> {
    pub fn new(
        key: impl Into<String>,
        state: &'a ModelSelectorState,
        options: &'a [ModelOption],
        access: impl Fn(&mut State) -> Option<&mut ModelSelectorState> + 'static,
    ) -> Self {
        Self {
            key: key.into(),
            label: "Model".to_owned(),
            state,
            options,
            current_model: None,
            mark_model: None,
            placeholder: "Select model".to_owned(),
            empty_message: "No matching models".to_owned(),
            reasoning_levels: &[],
            current_reasoning: None,
            placement: Placement::TopStart,
            trigger_width: None,
            popover_width: POPOVER_WIDTH,
            disabled: false,
            access: Rc::new(access),
            on_choose_model: None,
            on_choose_reasoning: None,
        }
    }

    pub fn label(mut self, label: impl Into<String>) -> Self {
        self.label = label.into();
        self
    }

    pub fn current_model(mut self, id: Option<&'a str>) -> Self {
        self.current_model = id;
        self
    }

    pub fn mark_model(mut self, id: Option<&'a str>) -> Self {
        self.mark_model = id;
        self
    }

    pub fn placeholder(mut self, text: impl Into<String>) -> Self {
        self.placeholder = text.into();
        self
    }

    pub fn empty_message(mut self, text: impl Into<String>) -> Self {
        self.empty_message = text.into();
        self
    }

    pub fn reasoning(mut self, levels: &'a [String], current: Option<&'a str>) -> Self {
        self.reasoning_levels = levels;
        self.current_reasoning = current;
        self
    }

    pub fn placement(mut self, placement: Placement) -> Self {
        self.placement = placement;
        self
    }

    pub fn trigger_width(mut self, width: f32) -> Self {
        self.trigger_width = Some(width);
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_choose_model(mut self, f: impl Fn(&mut State, &str, &mut Tx) + 'static) -> Self {
        self.on_choose_model = Some(Rc::new(f));
        self
    }

    pub fn on_choose_reasoning(mut self, f: impl Fn(&mut State, &str) + 'static) -> Self {
        self.on_choose_reasoning = Some(Rc::new(f));
        self
    }

    pub fn build(self, k: &Kit, layer: &mut Vec<El>) -> El {
        let selected = self
            .current_model
            .and_then(|id| self.options.iter().find(|o| o.id == id));
        let model_label = selected
            .map(|o| o.label.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| self.placeholder.clone());
        let has_slider = self.on_choose_reasoning.is_some() && self.reasoning_levels.len() > 1;
        let committed = self
            .current_reasoning
            .and_then(|c| self.reasoning_levels.iter().position(|l| l == c));
        let active = self
            .state
            .drag
            .filter(|&i| i < self.reasoning_levels.len())
            .or(committed);
        let effort_label = active.map(|i| reasoning_name(&self.reasoning_levels[i]));
        let open = self.state.open && !self.disabled;
        let anchor_key = format!("{}-anchor", self.key);
        let muted_trigger = self.mark_model.is_none();

        let mut trigger_content = row().gap(6.0).center_y().min_w(0.0);
        if self.trigger_width.is_some() {
            trigger_content = trigger_content.grow(1.0);
        } else {
            trigger_content = trigger_content.shrink();
        }
        if let Some(mark_id) = self.mark_model {
            trigger_content = trigger_content.child(
                icon(icons::mark(mark_id), 13.0)
                    .color(k.c.icon_muted)
                    .group_color(k.c.text_base),
            );
        }
        trigger_content = trigger_content.child(
            text(model_label.clone())
                .font(k.ty.text_xsmall_medium)
                .color(if muted_trigger {
                    k.c.text_muted
                } else {
                    k.c.text_base
                })
                .truncate(),
        );
        if has_slider && let Some(ref effort) = effort_label {
            trigger_content = trigger_content.child(
                text(effort.clone())
                    .key(format!("{}-trigger-effort", self.key))
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_muted),
            );
        }

        let mut trigger = row()
            .key(format!("{}-trigger", self.key))
            .role("combobox")
            .label(self.label.clone())
            .expanded(open)
            .disabled(self.disabled)
            .h(TRIGGER_H)
            .px(10.0)
            .gap(6.0)
            .radius(TRIGGER_H / 2.0)
            .center_y()
            .min_w(0.0)
            .fill(if open { k.c.layer_03 } else { k.c.layer_02 })
            .border(
                1.0,
                if open {
                    k.c.border_muted
                } else {
                    k.c.border_weak
                },
            )
            .hover_group()
            .child(trigger_content)
            .child(
                icon(k.glyph.chevron_down, 11.0)
                    .color(k.c.icon_muted)
                    .group_color(k.c.text_base),
            );
        if let Some(w) = self.trigger_width {
            trigger = trigger.w(w);
        } else {
            trigger = trigger.max_w(260.0).shrink();
        }
        if self.disabled {
            trigger = trigger.layer_opacity(0.5);
        } else {
            let access = self.access.clone();
            let current = self.current_model.map(str::to_owned);
            let key = self.key.clone();
            trigger = trigger
                .hover(k.c.layer_03)
                .focus_ring(2.0, 1.0, k.c.border_focus)
                .on_press_tx(move |s: &mut State, tx: &mut Tx| {
                    let Some(st) = access(s) else { return };
                    if st.open {
                        st.close();
                    } else if has_slider {
                        st.open_with(ModelSelectorStep::Effort, current.as_deref());
                    } else {
                        st.open_with(ModelSelectorStep::Models, current.as_deref());
                        if let Some(ref id) = current {
                            tx.scroll_into_view(format!("{key}-option-{id}"));
                        }
                    }
                });
        }

        let anchor = row()
            .key(anchor_key.clone())
            .min_w(0.0)
            .shrink()
            .child(trigger);

        if !open {
            return anchor;
        }

        let access_scrim = self.access.clone();
        layer.push(scrim(
            format!("{}-scrim", self.key),
            move |s: &mut State| {
                if let Some(st) = access_scrim(s) {
                    st.close();
                }
            },
        ));

        let step = if has_slider {
            self.state.step
        } else {
            ModelSelectorStep::Models
        };
        let content = match step {
            ModelSelectorStep::Effort => self.build_effort_card(
                k,
                model_label,
                committed,
                active,
                effort_label.unwrap_or_else(|| "—".to_owned()),
            ),
            ModelSelectorStep::Models => self.build_model_list(k, has_slider),
        };
        let surface = popover::raised(k, content, CARD_RADIUS);
        layer.push(popover::anchored(
            k,
            surface,
            &anchor_key,
            self.placement,
            6.0,
        ));

        anchor
    }

    fn build_effort_card(
        &self,
        k: &Kit,
        model_label: String,
        committed: Option<usize>,
        active: Option<usize>,
        effort_label: String,
    ) -> El {
        let access_models = self.access.clone();
        let current = self.current_model.map(str::to_owned);
        let key = self.key.clone();
        let mut header = row()
            .key(format!("{}-header", self.key))
            .role("button")
            .label(format!("Change model: {model_label}"))
            .w_full()
            .h(26.0)
            .px(4.0)
            .gap(6.0)
            .radius(7.0)
            .center_y()
            .hover(k.c.hover)
            .hover_group()
            .focus_ring(2.0, 1.0, k.c.border_focus);
        if let Some(mark_id) = self.mark_model {
            header = header.child(
                icon(icons::mark(mark_id), 13.0)
                    .color(k.c.icon_muted)
                    .group_color(k.c.text_base),
            );
        }
        header = header.child(
            row()
                .grow(1.0)
                .min_w(0.0)
                .center_y()
                .justify_center()
                .gap(5.0)
                .child(
                    text(model_label)
                        .font(k.ty.text_xsmall_medium)
                        .color(k.c.text_base)
                        .truncate(),
                )
                .child(
                    text(effort_label.clone())
                        .key(format!("{}-effort", self.key))
                        .font(k.ty.text_xsmall)
                        .color(k.c.text_muted),
                )
                .child(
                    icon(&icons::CHEV_RIGHT, 11.0)
                        .color(k.c.icon_muted)
                        .group_color(k.c.text_base),
                ),
        );
        if self.mark_model.is_some() {
            header = header.child(el().size(13.0, 13.0).a11y_hidden());
        }
        let header = header.on_press_tx(move |s: &mut State, tx: &mut Tx| {
            if let Some(st) = access_models(s) {
                st.show_models(current.as_deref());
            }
            if let Some(ref id) = current {
                tx.scroll_into_view(format!("{key}-option-{id}"));
            }
        });

        let slider = self.build_pill_slider(k, committed, active, effort_label);
        col()
            .key(format!("{}-content", self.key))
            .role("dialog")
            .label(self.label.clone())
            .w(self.popover_width)
            .padding(CARD_PAD)
            .gap(8.0)
            .child(header)
            .child(slider)
    }

    fn build_pill_slider(
        &self,
        k: &Kit,
        committed: Option<usize>,
        active: Option<usize>,
        display: String,
    ) -> El {
        let count = self.reasoning_levels.len();
        let track_w = (self.popover_width - 2.0 * CARD_PAD).max(PILL_H);
        let pad = PILL_H / 2.0;
        let travel = (track_w - PILL_H).max(1.0);
        let frac = if count > 1 {
            active.map_or(0.0, |i| i as f32 / (count - 1) as f32)
        } else {
            0.0
        };
        let has_fill = active.is_some_and(|i| i > 0);

        let mut track = el()
            .key(format!("{}-reasoning", self.key))
            .size(track_w, PILL_H)
            .radius(PILL_H / 2.0)
            .fill(k.c.layer_03)
            .border(1.0, k.alpha(k.c.std_border, 0.45))
            .role("slider")
            .label("Reasoning")
            .a11y_value(display)
            .range_value(
                active.unwrap_or(0) as f64,
                0.0,
                count.saturating_sub(1) as f64,
            )
            .range_step(1.0)
            .focus_ring(2.0, 1.0, k.c.border_focus)
            .child(
                el().key("fill")
                    .absolute(0.0, 0.0)
                    .size(
                        if has_fill {
                            PILL_H + frac * travel
                        } else {
                            PILL_H
                        },
                        PILL_H,
                    )
                    .radius(PILL_H / 2.0)
                    .fill(if has_fill {
                        k.c.bg_accent
                    } else {
                        Color::TRANSPARENT
                    })
                    .motion(k.motion.control)
                    .a11y_hidden(),
            );

        if count > 1 {
            for i in 0..count {
                let cx = pad + (i as f32 / (count - 1) as f32) * travel;
                let filled = active.is_some_and(|a| i < a);
                track = track.child(
                    el().key(format!("stop-{i}"))
                        .absolute(cx - PILL_DOT / 2.0, (PILL_H - PILL_DOT) / 2.0)
                        .size(PILL_DOT, PILL_DOT)
                        .radius(PILL_DOT / 2.0)
                        .fill(if filled {
                            k.c.knob_on.alpha(0.75)
                        } else {
                            k.c.text_muted.alpha(0.65)
                        })
                        .a11y_hidden(),
                );
            }
        }

        if active.is_some() {
            let knob = el()
                .key("thumb")
                .size(PILL_THUMB, PILL_THUMB)
                .absolute(PILL_INSET + frac * travel, PILL_INSET)
                .radius(PILL_THUMB / 2.0)
                .fill(k.c.knob_on);
            track = track.child(
                k.elevate(knob, &k.elevation.elements)
                    .motion(k.motion.control)
                    .a11y_hidden(),
            );
        }

        if count > 1
            && let Some(choose) = self.on_choose_reasoning.clone()
        {
            let levels_drag = self.reasoning_levels.to_vec();
            let access_drag = self.access.clone();
            let choose_drag = choose.clone();
            track = track.touch_drags_at_once().on_drag(
                move |s: &mut State, d: &mut Drag, tx: &mut Tx| {
                    let idx = pill_index(d.x, d.w, levels_drag.len());
                    match d.phase {
                        Phase::Down => {
                            if let Some(st) = access_drag(s) {
                                st.drag = Some(idx);
                            }
                            tx.animated();
                        }
                        Phase::Move => {
                            if let Some(st) = access_drag(s) {
                                st.drag = Some(idx);
                            }
                        }
                        Phase::Up => {
                            let picked = access_drag(s).and_then(|st| st.drag.take());
                            if !d.cancelled()
                                && let Some(i) = picked
                                && Some(i) != committed
                                && let Some(level) = levels_drag.get(i)
                            {
                                choose_drag(s, level);
                            }
                        }
                    }
                },
            );

            let levels_step = self.reasoning_levels.to_vec();
            let choose_step = choose.clone();
            track = track.on_step(move |s: &mut State, delta: i32, tx: &mut Tx| {
                let base = committed.unwrap_or(0) as isize;
                let next =
                    (base + delta as isize).clamp(0, (levels_step.len() - 1) as isize) as usize;
                if Some(next) != committed {
                    tx.animated();
                    choose_step(s, &levels_step[next]);
                }
            });

            let levels_limit = self.reasoning_levels.to_vec();
            track = track.on_step_limit(move |s: &mut State, max: bool, tx: &mut Tx| {
                let next = if max { levels_limit.len() - 1 } else { 0 };
                if Some(next) != committed {
                    tx.animated();
                    choose(s, &levels_limit[next]);
                }
            });
        }

        track
    }

    fn build_model_list(&self, k: &Kit, has_slider: bool) -> El {
        let options = Rc::new(self.options.to_vec());
        let choices = filtered(&options, &self.state.search.text);
        let active_orig = active_index(&options, &choices, self.state.highlight.as_deref());
        let reasoning_enabled = self.on_choose_reasoning.is_some();

        let access_edit = self.access.clone();
        let access_change = self.access.clone();
        let options_change = options.clone();
        let key_change = self.key.clone();

        let access_key = self.access.clone();
        let options_key = options.clone();
        let key_nav = self.key.clone();
        let choose_key = self.on_choose_model.clone();

        let mut search = TextInput::new(&self.state.search)
            .plain()
            .leading_icon(k.glyph.search)
            .height(30.0)
            .width(None)
            .placeholder("Search models")
            .label(format!("Search {}", self.label))
            .on_edit(move |s: &mut State| {
                if access_edit(s).is_some() {
                    &mut access_edit(s)
                        .expect("just verified selector state exists")
                        .search
                } else {
                    &mut s.scratch_search
                }
            })
            .on_change(move |s: &mut State, tx: &mut Tx| {
                let Some(st) = access_change(s) else { return };
                st.highlight = None;
                if let Some(&first) = filtered(&options_change, &st.search.text).first() {
                    tx.scroll_into_view(format!(
                        "{key_change}-option-{}",
                        options_change[first].id
                    ));
                }
            })
            .on_key(move |s: &mut State, ev: &KeyEvent, tx: &mut Tx| {
                handle_search_key(
                    s,
                    &access_key,
                    &options_key,
                    &key_nav,
                    reasoning_enabled,
                    choose_key.as_ref(),
                    ev,
                    tx,
                )
            });
        if let Some(orig) = active_orig {
            search =
                search.active_descendant(Some(format!("{}-option-{}", self.key, options[orig].id)));
        }
        let search = search.build(k).key(format!("{}-search", self.key));

        let mut header = row().w_full().px(6.0).center_y().gap(2.0);
        if has_slider {
            let access_back = self.access.clone();
            header = header.child(
                IconButton::new(&icons::BACK)
                    .variant(IconButtonVariant::GhostMuted)
                    .control_size(ControlSize::Small)
                    .label("Back")
                    .on_press(move |s: &mut State| {
                        if let Some(st) = access_back(s) {
                            st.show_effort();
                        }
                    })
                    .build(k)
                    .key(format!("{}-back", self.key)),
            );
        }
        header = header.child(row().grow(1.0).min_w(0.0).child(search));

        let list = if choices.is_empty() {
            let message = if options.is_empty() {
                self.empty_message.clone()
            } else {
                "No matching models".to_owned()
            };
            col().w_full().px(8.0).py(8.0).child(
                text(message)
                    .font(k.ty.text_xsmall)
                    .color(k.c.text_muted)
                    .wrap(),
            )
        } else {
            let count = choices.len();
            let choices = Rc::new(choices);
            let options_key = options.clone();
            let choices_key = choices.clone();
            let key_for_id = self.key.clone();

            let options_row = options;
            let choices_row = choices;
            let key_prefix = self.key.clone();
            let current_model = self.current_model.map(str::to_owned);
            let access_row = self.access.clone();
            let choose_row = self.on_choose_model.clone();
            let kit = k.clone();

            popover::fit_scroll(
                col()
                    .w_full()
                    .growth(rune::GrowthSpec::default())
                    .virtual_rows::<State>(
                        count,
                        ROW_H,
                        move |_, pos| {
                            let orig = choices_key[pos];
                            std::borrow::Cow::Owned(format!(
                                "{key_for_id}-option-{}",
                                options_key[orig].id
                            ))
                        },
                        move |_, pos| {
                            let orig = choices_row[pos];
                            let option = &options_row[orig];
                            let is_active = active_orig == Some(orig);
                            let is_selected = current_model.as_deref() == Some(option.id.as_str());
                            let id_hover = option.id.clone();
                            let id_press = option.id.clone();
                            let option_has_reasoning = option.has_reasoning;
                            let access_hover = access_row.clone();
                            let access_press = access_row.clone();
                            let choose_press = choose_row.clone();

                            let mut line = row()
                                .w_full()
                                .h(ROW_H)
                                .px(8.0)
                                .gap(8.0)
                                .center_y()
                                .radius(6.0)
                                .fill(if is_active {
                                    kit.c.hover
                                } else {
                                    Color::TRANSPARENT
                                });
                            if let Some(ref mark_id) = option.mark_id {
                                line = line.child(
                                    icon(icons::mark(mark_id), 13.0).color(kit.c.icon_muted),
                                );
                            }
                            let mut labels = row().grow(1.0).min_w(0.0).gap(6.0).center_y().child(
                                text(&option.label)
                                    .font(kit.ty.text_xsmall)
                                    .color(kit.c.text_base)
                                    .truncate(),
                            );
                            if !option.detail.is_empty() {
                                labels = labels.child(
                                    text(&option.detail)
                                        .font(kit.ty.text_xsmall)
                                        .color(kit.c.text_faint)
                                        .truncate(),
                                );
                            }
                            line = line.child(labels).child(
                                icon(kit.glyph.check_small, 12.0)
                                    .color(kit.c.icon_base)
                                    .opacity(if is_selected { 1.0 } else { 0.0 }),
                            );
                            kit.interactive(line)
                                .key(format!("{key_prefix}-option-{}", option.id))
                                .role("option")
                                .label(&option.label)
                                .focus_selected(is_active)
                                .position_in_set(pos + 1, count)
                                .on_hover(move |s: &mut State| {
                                    if let Some(st) = access_hover(s) {
                                        st.highlight = Some(id_hover.clone());
                                    }
                                })
                                .on_press_tx(move |s: &mut State, tx: &mut Tx| {
                                    choose_model_option(
                                        s,
                                        &access_press,
                                        choose_press.as_ref(),
                                        &id_press,
                                        reasoning_enabled && option_has_reasoning,
                                        tx,
                                    );
                                })
                        },
                    ),
            )
            .h((count as f32 * ROW_H).min(LIST_MAX_H))
            .min_h(ROW_H)
            .role("listbox")
            .label(self.label.clone())
        }
        .preserve_focus_on_press(true);

        col()
            .key(format!("{}-content", self.key))
            .role("dialog")
            .label(self.label.clone())
            .w(self.popover_width)
            .shrink()
            .min_h(0.0)
            .child(header)
            .child(el().w_full().h(1.0).fill(k.c.border_weak).a11y_hidden())
            .child(col().w_full().padding(4.0).shrink().min_h(0.0).child(list))
    }
}

fn choose_model_option(
    s: &mut State,
    access: &SelectorAccess,
    choose: Option<&ModelChoose>,
    id: &str,
    show_effort: bool,
    tx: &mut Tx,
) {
    if let Some(choose) = choose {
        choose(s, id, tx);
    }
    if let Some(st) = access(s) {
        if show_effort {
            st.show_effort();
        } else {
            st.close();
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "keyboard handler needs state accessor, options, key, callbacks, and event"
)]
fn handle_search_key(
    s: &mut State,
    access: &SelectorAccess,
    options: &[ModelOption],
    key: &str,
    reasoning_enabled: bool,
    choose: Option<&ModelChoose>,
    ev: &KeyEvent,
    tx: &mut Tx,
) -> bool {
    let Some(st) = access(s) else {
        return false;
    };
    if st.search.preedit().is_some() {
        return false;
    }
    if ev.key == Key::Escape {
        if !tx.key_repeat() {
            st.close();
        }
        return true;
    }
    let choices = filtered(options, &st.search.text);
    let active = active_index(options, &choices, st.highlight.as_deref());
    let control = ev.mods.ctrl && !ev.mods.shift && !ev.mods.alt && !ev.mods.logo;
    let move_delta = match &ev.key {
        Key::Down if !ev.mods.cmd && !ev.mods.ctrl => Some(1isize),
        Key::Up if !ev.mods.cmd && !ev.mods.ctrl => Some(-1isize),
        Key::Text(c) if control && c == "n" => Some(1),
        Key::Text(c) if control && c == "p" => Some(-1),
        Key::Command('n') if control => Some(1),
        Key::Command('p') if control => Some(-1),
        _ => None,
    };
    if let Some(delta) = move_delta {
        if !choices.is_empty() {
            let pos = active
                .and_then(|orig| choices.iter().position(|&c| c == orig))
                .unwrap_or(0);
            let next = (pos as isize + delta).rem_euclid(choices.len() as isize) as usize;
            let id = options[choices[next]].id.clone();
            st.highlight = Some(id.clone());
            tx.scroll_into_view(format!("{key}-option-{id}"));
        }
        return true;
    }
    if ev.key == Key::Enter && !ev.mods.shift {
        if tx.key_repeat() {
            return true;
        }
        let picked = active.map(|i| (options[i].id.clone(), options[i].has_reasoning));
        if let Some((id, has_reasoning)) = picked {
            choose_model_option(
                s,
                access,
                choose,
                &id,
                reasoning_enabled && has_reasoning,
                tx,
            );
        } else {
            st.close();
        }
        return true;
    }
    false
}

fn active_index(
    options: &[ModelOption],
    choices: &[usize],
    highlight: Option<&str>,
) -> Option<usize> {
    if let Some(id) = highlight
        && let Some(orig) = choices.iter().copied().find(|&i| options[i].id == id)
    {
        return Some(orig);
    }
    choices.first().copied()
}

fn score(query: &str, value: &str) -> Option<i32> {
    let needle: Vec<_> = query.chars().collect();
    let mut n = 0;
    let mut score = 0;
    let mut previous = false;
    let mut boundary = true;
    for c in value.to_lowercase().chars() {
        if n < needle.len() && c == needle[n] {
            score += 10 + if previous { 12 } else { 0 } + if boundary { 8 } else { 0 };
            previous = true;
            n += 1;
        } else {
            previous = false;
            score -= 1;
        }
        boundary = !c.is_alphanumeric();
    }
    (n == needle.len()).then_some(score)
}

fn filtered(options: &[ModelOption], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return (0..options.len()).collect();
    }
    let mut matches: Vec<_> = options
        .iter()
        .enumerate()
        .filter_map(|(i, o)| {
            score(&query, &o.label)
                .into_iter()
                .chain(score(&query, &o.detail))
                .max()
                .map(|s| (i, s))
        })
        .collect();
    matches.sort_by_key(|(i, s)| (std::cmp::Reverse(*s), *i));
    matches.into_iter().map(|(i, _)| i).collect()
}

fn pill_index(x: f32, width: f32, count: usize) -> usize {
    if count <= 1 {
        return 0;
    }
    let pad = PILL_H / 2.0;
    let travel = (width - PILL_H).max(1.0);
    let frac = ((x - pad) / travel).clamp(0.0, 1.0);
    (frac * (count - 1) as f32).round() as usize
}
