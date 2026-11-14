//! A chat's page, as the phone shows it: the header, the transcript with its
//! tool groups, thinking and errors, pi's dialogs, and the composer under
//! them.

use rune::{Color, El, Link, ScrollSpec, Span, TextStyle, Tx, col, el, icon, rich, row, text};
use rune_kit::markdown::MarkdownStyle;
use rune_kit::{
    Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Markdown, Menu,
    MenuItem, TextArea, TextInput,
};

use super::{breaks, code, icons};

use super::widgets::{busy, dot, engine_note, head, head_titles, hide_button, tones};
use crate::app::{Answer, ChatPage, Existing, Overlay, QuestionForm, State, Target, Window};
use crate::data::{DialogKind, Message, MessageStatus, Part, Role, Run, ToolState};
use crate::labels::{self, ToolCall, tool_summary};
use crate::message::skill_words;
use rune::Image;
use rune_theme_opencode::file_icons;

/// How wide the main window's transcript and composer get, so lines stay
/// readable in a wide window.
const READING_WIDTH: f32 = 780.0;

/// The quick window's chat page: its header over the page.
pub fn view(s: &State, page: &ChatPage, k: &Kit, layer: &mut Vec<El>) -> El {
    if let Some(chat) = page.existing()
        && let Some(changes) = &chat.changes
    {
        return super::changes::page(s, chat, changes, k);
    }
    col()
        .w_full()
        .h_full()
        .child(header(page, k, layer))
        .child(body(s, page, k, layer))
}

/// The main window's chat pane: the page without a header, as the
/// window's toolbar has the title and actions.
pub fn pane(s: &State, page: &ChatPage, k: &Kit, layer: &mut Vec<El>) -> El {
    if let Some(chat) = page.existing()
        && let Some(changes) = &chat.changes
    {
        return super::changes::page(s, chat, changes, k);
    }
    body(s, page, k, layer)
}

/// The transcript, or a new chat's greeting, over the composer.
fn body(s: &State, page: &ChatPage, k: &Kit, layer: &mut Vec<El>) -> El {
    let wide = s.window == Window::Main;
    let mut body = col().key("transcript").w_full().px(14.0).py(10.0).gap(14.0);
    if wide {
        body = body.max_w(READING_WIDTH);
    }
    let error = page.existing().and_then(|chat| {
        chat.error.as_deref().filter(|why| {
            !chat.messages().any(|message| {
                message
                    .parts
                    .iter()
                    .any(|part| matches!(part, Part::Error { error } if error == why))
            })
        })
    });
    if let Some(note) = engine_note(s, k) {
        body = body.child(note);
    } else if let Some(e) = error {
        body = body.child(errbox(k, e));
    }
    if !page.can_send() {
        body = body.child(row().w_full().gap(10.0).padding(10.0).center_y().radius(10.0).fill(k.c.warning.bg)
            .child(text("Open in terminal pi. Stop it there before continuing here to send or change models.")
                .font(k.ty.ui).color(k.c.text_base).wrap().grow(1.0).min_w(0.0))
            .child(Button::new("Continue here").variant(ButtonVariant::Outline).on_press(State::take_over).build(k)));
    }
    body = match &page.target {
        Target::New(_) => body.child(hero(k)),
        Target::Existing(chat) => {
            let expanded = s.expand_tools.get();
            body.children(transcript(chat, k, expanded))
        }
    };
    let mut scroll = col()
        .key("transcript")
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_anchor()
        .on_far_from_bottom(160.0, |s: &mut State, far| {
            if let Some(page) = s.chat_mut() {
                page.far_from_bottom = far;
            }
        })
        .child(body.key("messages"));
    if wide {
        scroll = scroll.items_center();
    }
    let mut stack = rune::el().w_full().grow(1.0).min_h(0.0).child(scroll);
    if page.far_from_bottom {
        let jump = IconButton::new(&icons::CHEV_DOWN)
            .variant(IconButtonVariant::Neutral)
            .label("Scroll to bottom")
            .on_press_tx(|_: &mut State, tx: &mut Tx| tx.scroll_to_bottom("transcript"))
            .build(k);
        stack = stack.child(
            row()
                .pin(None, None, Some(0.0), Some(10.0))
                .w_full()
                .justify_center()
                .child(jump),
        );
    }
    let mut composer = super::composer::view(s, page, k, layer);
    if wide {
        composer = row()
            .w_full()
            .justify_center()
            .child(composer.max_w(READING_WIDTH));
    }
    col()
        .w_full()
        .grow(1.0)
        .min_h(0.0)
        .child(stack)
        .child(composer)
}

/// The page's title and subtitle: the chat's title and project.
pub fn titles(page: &ChatPage) -> (String, String) {
    match &page.target {
        Target::New(d) => {
            let project = d
                .project
                .as_ref()
                .and_then(|id| d.projects.iter().find(|p| &p.id == id));
            (
                "New chat".to_owned(),
                project.map(|p| p.name.clone()).unwrap_or_default(),
            )
        }
        Target::Existing(Existing { chat: Some(c), .. }) => (
            c.title().to_owned(),
            c.project
                .as_ref()
                .map(|p| p.name.clone())
                .unwrap_or_default(),
        ),
        Target::Existing(_) => (String::new(), String::new()),
    }
}

fn header(page: &ChatPage, k: &Kit, layer: &mut Vec<El>) -> El {
    let (title, sub) = titles(page);
    let back = IconButton::new(&icons::BACK)
        .variant(IconButtonVariant::Ghost)
        .label("Back")
        .help("Back (⌘[)")
        .on_press(State::back)
        .build(k);
    let titles = head_titles()
        .gap(1.0)
        .child(
            text(title)
                .font(k.ty.ui_medium)
                .color(k.c.text_base)
                .truncate(),
        )
        .child(
            text(sub)
                .font(k.ty.text_xsmall)
                .color(k.c.text_faint)
                .truncate(),
        );
    let mut bar = head("chat-head").child(back).child(titles);
    if let Some(chat) = page.existing() {
        let id = chat.id.clone();
        bar = bar
            .child(
                IconButton::new(&icons::EXPAND)
                    .variant(IconButtonVariant::Ghost)
                    .label("Open in the main window")
                    .help("Open in the main window")
                    .on_press(move |s: &mut State| s.open_in_peer(id.clone()))
                    .build(k),
            )
            .child(more_menu(page, k, layer));
    }
    bar = bar.child(hide_button(k));
    bar
}

/// The chat's menu: agent controls and session actions.
pub fn more_menu(page: &ChatPage, k: &Kit, layer: &mut Vec<El>) -> El {
    let trigger = IconButton::new(&icons::MORE)
        .variant(IconButtonVariant::Ghost)
        .label("More")
        .expanded(page.overlay == Overlay::Menu)
        .on_press(|s: &mut State| {
            if let Some(p) = s.chat_mut() {
                p.overlay = match p.overlay {
                    Overlay::Menu => Overlay::Closed,
                    Overlay::Closed => Overlay::Menu,
                };
            }
        })
        .build(k);
    let mut menu = Menu::new("chat-menu")
        .trigger(trigger)
        .open(page.overlay == Overlay::Menu)
        .on_close(close_menu);
    if page.is_working() {
        menu = menu.item(
            MenuItem::new("Stop the agent")
                .icon(&icons::STOP)
                .danger(true)
                .on_select(|s: &mut State| {
                    close_menu(s);
                    s.stop();
                }),
        );
    }
    let Some(chat) = page.existing() else {
        return menu.build(k, layer);
    };
    let (rename, delete) = (chat.id.clone(), chat.id.clone());
    let copy = chat.id.clone();
    menu.item(
        MenuItem::new("Copy Chat ID")
            .icon(&icons::COPY)
            .on_select_tx(move |s: &mut State, tx| {
                close_menu(s);
                tx.copy(&copy);
            }),
    )
    .item(
        MenuItem::new("Rename…")
            .icon(&icons::EDIT)
            .on_select(move |s: &mut State| {
                close_menu(s);
                s.ask_rename(rename.clone());
            }),
    )
    .item(
        MenuItem::new("Delete…")
            .icon(&icons::DELETE)
            .danger(true)
            .on_select(move |s: &mut State| {
                close_menu(s);
                s.ask_delete(delete.clone());
            }),
    )
    .build(k, layer)
}

fn close_menu(s: &mut State) {
    if let Some(p) = s.chat_mut().filter(|p| p.overlay == Overlay::Menu) {
        p.overlay = Overlay::Closed;
    }
}

fn hero(k: &Kit) -> El {
    col().w_full().pt(80.0).items_center().child(
        text("What are we building?")
            .font(TextStyle {
                size: 20.0,
                ..k.ty.body_medium
            })
            .color(k.c.text_muted),
    )
}

fn errbox(k: &Kit, why: &str) -> El {
    row()
        .w_full()
        .gap(8.0)
        .padding(10.0)
        .radius(10.0)
        .fill(k.c.danger.bg)
        .child(icon(&icons::ALERT, 14.0).color(k.c.danger.icon))
        .child(
            text(why.to_owned())
                .font(k.ty.ui)
                .color(k.c.text_base)
                .wrap()
                .grow(1.0)
                .min_w(0.0),
        )
}

/// The chat's messages; `expanded` opens tool groups until clicked.
fn transcript(page: &Existing, k: &Kit, expanded: bool) -> Vec<El> {
    let Some(shown) = &page.page else {
        return vec![
            row()
                .w_full()
                .py(32.0)
                .justify_center()
                .child(busy(18.0, k)),
        ];
    };
    let working = page.chat.as_ref().is_some_and(|c| c.is_working());
    let style = code::markdown_style(k);
    let mut out = Vec::new();
    if page.older_remaining > 0 {
        out.push(
            row().w_full().justify_center().child(
                Button::new("Load earlier messages")
                    .variant(ButtonVariant::Ghost)
                    .on_press(State::load_earlier)
                    .build(k),
            ),
        );
    }
    let last = shown.last().map(|m| m.id.as_str());
    // When the turn of the reply next shown began: its prompt's time.
    let mut started = None;
    for m in page.messages() {
        let key = format!("m-{}", m.id);
        match m.role {
            Role::User => {
                let images = m
                    .images()
                    .map(|i| {
                        let thumb = page.thumbs.get(&(m.id.clone(), i)).and_then(Option::as_ref);
                        (thumb, m.written().then(|| (m.id.clone(), i)))
                    })
                    .collect();
                started = Some(m.created_at);
                out.push(user_bubble(k, &m.text(), images, Shade::Sent).key(key));
            }
            Role::Assistant => {
                let streaming = m.status == MessageStatus::Streaming
                    || (Some(m.id.as_str()) == last && working);
                let started = started.take().unwrap_or(m.created_at);
                out.push(assistant(page, k, &style, m, started, streaming, expanded).key(key));
            }
            Role::System => out.push(system_note(k, m).key(key)),
        }
    }
    for (i, p) in page.pending.iter().enumerate() {
        let images = p.images.iter().map(|t| (t.as_ref(), None)).collect();
        out.push(user_bubble(k, &p.text, images, Shade::Pending).key(format!("pending-{i}")));
    }
    let queued = page
        .run
        .iter()
        .flat_map(|r| r.steering.iter().chain(&r.follow_up));
    for (i, text) in queued.enumerate() {
        out.push(user_bubble(k, text, Vec::new(), Shade::Queued).key(format!("queued-{i}")));
    }
    if let Some(note) = run_note(k, page.run.as_ref()) {
        out.push(note.key("run-note"));
    } else if page.shows_clock() {
        let since = page
            .messages()
            .filter(|m| m.role == Role::User)
            .last()
            .map(|m| m.created_at);
        let secs = since.map(|t| (chrono::Utc::now().timestamp_millis() - t).max(0) / 1000);
        out.push(working_row(k, "Working", secs.map(|s| format!("{s}s"))).key("working"));
    }
    if let Some(form) = &page.question {
        out.push(question(k, form).key(format!("q-{}", form.dialog.id)));
    }
    out
}

/// A spinner with what the run does, and a faint detail after it.
fn working_row(k: &Kit, label: &str, detail: Option<String>) -> El {
    row()
        .gap(8.0)
        .center_y()
        .child(busy(12.0, k))
        .child(text(label.to_owned()).font(k.ty.ui).color(k.c.text_muted))
        .child(
            text(detail.unwrap_or_default())
                .font(k.ty.ui)
                .color(k.c.text_faint)
                .truncate()
                .min_w(0.0),
        )
}

/// What a run waits on besides the model: a retry after an error, or
/// compacting the context.
fn run_note(k: &Kit, run: Option<&Run>) -> Option<El> {
    let run = run?;
    if let Some(retry) = &run.retry {
        let wait = retry
            .at
            .map(|at| (at - chrono::Utc::now()).num_seconds())
            .filter(|s| *s > 0)
            .map(|s| format!("in {s}s"));
        let label = format!("Retry {} of {}", retry.attempt, retry.max_attempts);
        let mut out = working_row(k, &label, wait);
        if let Some(why) = &retry.error {
            out = out.help(why.clone());
        }
        return Some(out);
    }
    run.compacting
        .as_ref()
        .map(|_| working_row(k, "Compacting context", None))
}

/// How a user bubble shows: sent, sending, or waiting in pi's queue.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Shade {
    Sent,
    Pending,
    Queued,
}

/// An image in a user bubble: its thumbnail once loaded, and the message
/// and index that open it once pi has written it.
type Tile<'a> = (Option<&'a Image>, Option<(String, usize)>);

/// A message the user sent, with its images.
fn user_bubble(k: &Kit, text_in: &str, images: Vec<Tile>, shade: Shade) -> El {
    let opacity = if shade == Shade::Sent { 1.0 } else { 0.6 };
    // A skill the message ran stands out; the rest stays literal text.
    let mut shown = String::new();
    let mut spans = Vec::new();
    for (piece, is_skill) in skill_words(text_in.trim()) {
        let start = shown.len();
        shown.push_str(piece);
        if is_skill {
            spans.push(
                Span::new(start, shown.len())
                    .color(k.c.text_accent)
                    .weight(560.0),
            );
        }
    }
    spans.extend(super::widgets::markdown_spans(k, &shown));
    let links = crate::markup::links(&shown);
    let mut out = col().w_full().pl(40.0).items_end().gap(4.0);
    if !images.is_empty() {
        out = out.child(row().gap(6.0).children(images.into_iter().enumerate().map(
            |(i, (thumb, open))| {
                let tile = super::composer::thumb(k, thumb, 72.0).opacity(opacity);
                match open {
                    Some((message, index)) => k
                        .interactive(tile)
                        .key(format!("img-{message}-{index}"))
                        .label("Open the image")
                        .on_press(move |s: &mut State| s.open_image(message.clone(), index)),
                    None => tile.key(format!("img-{i}")),
                }
            },
        )));
    }
    if !shown.is_empty() {
        let mut body = rich(shown, spans)
            .font(k.ty.ui)
            .color(k.c.text_base)
            .wrap_anywhere()
            .selectable(true);
        for (range, url) in links {
            body = body.link(Link::new(range, url));
        }
        body = body.on_link(|s: &mut State, url: &str, tx: &mut Tx| s.open_link(url, tx));
        out = out.child(
            col()
                .shrink()
                .min_w(0.0)
                .px(12.0)
                .py(8.0)
                .radius(14.0)
                .fill(k.c.layer_03)
                .opacity(opacity)
                .child(body),
        );
    }
    if shade == Shade::Queued {
        out = out.child(text("Queued").font(k.ty.text_xsmall).color(k.c.text_faint));
    }
    out
}

/// A row that's neither side's, like a compaction, in the middle.
fn system_note(k: &Kit, m: &Message) -> El {
    note(k, &m.text())
}

/// A one-line note in the middle, faint.
fn note(k: &Kit, line: &str) -> El {
    row().w_full().justify_center().child(
        text(single_line(line))
            .font(k.ty.small)
            .color(k.c.text_faint)
            .truncate()
            .min_w(0.0),
    )
}

/// A piece of an assistant message as the transcript shows it: the work
/// between two texts, tool calls and thinking, goes in one group.
enum Piece<'a> {
    Part(usize, &'a Part),
    Work(usize, Vec<Step<'a>>),
}

/// One step of the work between two texts, by its part's index.
enum Step<'a> {
    Tool(ToolCall<'a>),
    Thought(usize, &'a str),
}

fn pieces(parts: &[Part]) -> Vec<Piece<'_>> {
    let mut out: Vec<Piece> = Vec::new();
    for (i, part) in parts.iter().enumerate() {
        let step = match part {
            Part::Tool {
                id,
                name,
                subject,
                state,
                calls,
            } => Step::Tool(ToolCall {
                id,
                name,
                subject: subject.as_deref().unwrap_or_default(),
                state: *state,
                calls,
            }),
            Part::Thinking { text } => Step::Thought(i, text),
            // The transcript shows a reply's images nowhere, so they don't split the work.
            Part::Image { .. } => continue,
            Part::Text { .. } | Part::Error { .. } | Part::Note { .. } => {
                out.push(Piece::Part(i, part));
                continue;
            }
        };
        match out.last_mut() {
            Some(Piece::Work(_, steps)) => steps.push(step),
            _ => out.push(Piece::Work(i, vec![step])),
        }
    }
    out
}

/// Prose, thinking, tool groups and errors in transcript order, as on the
/// desktop. A finished reply folds everything before its last text under how
/// long the turn, begun at `started`, worked. While it streams, the work at
/// its end opens if `expanded`; work with a text after it starts closed.
fn assistant(
    page: &Existing,
    k: &Kit,
    style: &MarkdownStyle,
    m: &Message,
    started: i64,
    streaming: bool,
    expanded: bool,
) -> El {
    let mut out = col().w_full().gap(10.0);
    let last = m.parts.len().saturating_sub(1);
    let pieces = pieces(&m.parts);
    let tail = pieces.len().saturating_sub(1);
    let answer = pieces
        .iter()
        .rposition(|p| matches!(p, Piece::Part(_, Part::Text { .. })));
    let fold = answer.filter(|&n| n > 0 && !streaming);
    let worked_key = format!("{}#worked", m.id);
    let worked_open = is_open(page, &worked_key, false);
    if fold.is_some() {
        out = out.child(worked(k, &worked_key, m.ended_at - started, worked_open));
    }
    for (n, piece) in pieces.into_iter().enumerate() {
        if fold.is_some_and(|f| n < f) && !worked_open {
            continue;
        }
        // The rule sets the answer apart from the work, shown or not.
        if Some(n) == fold {
            out = out.child(el().w_full().h(1.0).fill(k.c.border_muted));
        }
        out = out.children(match piece {
            Piece::Part(i, Part::Text { text }) => vec![
                prose(page, k, style, text, streaming && i == last).key(format!("{}#{i}", m.id)),
            ],
            Piece::Part(_, Part::Error { error }) => vec![errbox(k, error)],
            Piece::Part(_, Part::Note { text }) => vec![note(k, text)],
            Piece::Part(..) => Vec::new(),
            // Thinking alone stays a fold of its own; with tool calls it joins their rail.
            Piece::Work(_, steps) if !steps.iter().any(|s| matches!(s, Step::Tool(_))) => steps
                .iter()
                .filter_map(|step| match step {
                    Step::Thought(i, text) => Some(thinking(
                        page,
                        k,
                        &format!("{}#{i}", m.id),
                        text,
                        streaming && *i == last,
                    )),
                    Step::Tool(_) => None,
                })
                .collect(),
            Piece::Work(i, steps) => {
                let key = format!("{}#{i}", m.id);
                let live = streaming.then_some(last);
                let open = streaming && expanded && n == tail;
                vec![tool_group(page, k, &key, &steps, live, open)]
            }
        });
    }
    if m.status == MessageStatus::Aborted {
        out = out.child(text("Stopped").font(k.ty.small).color(k.c.text_faint));
    }
    if !streaming {
        out = out.child(footer(page, k, m));
    }
    out
}

/// A text part as Markdown, with its inline code coloured and linked to the
/// files it names.
fn prose(page: &Existing, k: &Kit, style: &MarkdownStyle, text: &str, streaming: bool) -> El {
    let soft = breaks::breakable(text);
    let shown = soft.as_deref().unwrap_or(text);
    let links = page
        .chat
        .as_ref()
        .map(|c| code::file_links(&c.cwd, &page.file_links));
    let link = |code: &str| links.as_ref().and_then(|f| f(code));
    let mut md = Markdown::new(shown.to_owned())
        .style(style.clone())
        .highlights(code::inline_code_marks(k, shown, link))
        .streaming(streaming)
        .selectable(true)
        // The source with break points would paste broken links.
        .copy_source(soft.is_none())
        .on_link_tx(|s: &mut State, url: &str, tx: &mut Tx| {
            // Code links are paths; Rune opens web links only.
            s.open_link(url, tx);
        });
    if let Some(links) = links {
        md = md.code_link(k.c.text_accent, links);
    }
    let md = md.build(k);
    match soft {
        Some(_) => col()
            .w_full()
            .child(md)
            .selection_copy(|sel| Some(sel.text().replace(breaks::ZWSP, ""))),
        None => md,
    }
}

/// The header that folds a finished turn's work before its answer, with how
/// long the turn took.
fn worked(k: &Kit, key: &str, millis: i64, open: bool) -> El {
    let toggle = key.to_owned();
    let label = match millis {
        1.. => format!("Worked for {}", labels::worked(millis / 1000)),
        _ => "Worked".to_owned(),
    };
    let head = row()
        .h(TOOL_HEADER_HEIGHT)
        .gap(4.0)
        .pr(6.0)
        .center_y()
        .child(text(label).font(k.ty.ui).color(k.c.text_faint))
        .child(chevron(k, open));
    let head = k
        .interactive(head.radius(6.0).shrink())
        .label(if open {
            "Hide the work"
        } else {
            "Show the work"
        })
        .on_press(move |s: &mut State| toggle_group(s, &toggle, open));
    row().key(format!("worked-{key}")).w_full().child(head)
}

/// Whether the fold at `key` shows open: as the user last set it, or
/// `default`.
fn is_open(page: &Existing, key: &str, default: bool) -> bool {
    page.open_groups.get(key).copied().unwrap_or(default)
}

/// The model's reasoning, folded under "Thinking" until opened.
fn thinking(page: &Existing, k: &Kit, key: &str, text_in: &str, live: bool) -> El {
    let open = is_open(page, key, false);
    let toggle = key.to_owned();
    let mut head = row()
        .h(TOOL_HEADER_HEIGHT)
        .gap(RAIL_X)
        .pr(6.0)
        .center_y()
        .child(chevron(k, open))
        .child(
            text(if live { "Thinking" } else { "Thought" })
                .font(TextStyle {
                    size: TOOL_TEXT,
                    ..k.ty.ui
                })
                .color(k.c.text_muted),
        );
    if live {
        head = head.child(busy(11.0, k));
    }
    let head = k
        .interactive(head.radius(6.0).shrink())
        .on_press(move |s: &mut State| toggle_group(s, &toggle, open));
    let mut out = col()
        .key(format!("thinking-{key}"))
        .w_full()
        .child(row().w_full().child(head));
    if open {
        out = out.child(
            text(text_in.trim().to_owned())
                .font(TextStyle {
                    size: TOOL_TEXT,
                    line_height: 18.0,
                    ..k.ty.ui
                })
                .color(k.c.text_faint)
                .wrap()
                .selectable(true)
                .ml(RAIL_X * 3.0)
                .mt(4.0),
        );
    }
    out
}

fn chevron(k: &Kit, open: bool) -> El {
    icon(&icons::CHEV_DOWN, RAIL_X * 2.0)
        .color(k.c.icon_faint)
        .rotate(if open {
            0.0
        } else {
            -std::f32::consts::FRAC_PI_2
        })
}

/// Close the fold at `key` if it shows `open`, else open it, and keep it so.
fn toggle_group(s: &mut State, key: &str, open: bool) {
    if let Some(chat) = s.chat_mut().and_then(ChatPage::existing_mut) {
        chat.open_groups.insert(key.to_owned(), !open);
    }
}

/// Show a finished reply's timestamp, copy action and saved turn diff.
fn footer(page: &Existing, k: &Kit, m: &Message) -> El {
    let mut out = row().w_full().gap(8.0).center_y();
    if let Some(at) = chrono::DateTime::from_timestamp_millis(m.created_at) {
        out = out.child(
            text(
                at.with_timezone(&chrono::Local)
                    .format("%b %-d, %-I:%M %p")
                    .to_string(),
            )
            .font(k.ty.small)
            .color(k.c.text_faint),
        );
    }
    if m.has_answer() {
        let copied = page.just_copied(&m.id);
        let id = m.id.clone();
        out = out.child(
            IconButton::new(if copied { &icons::CHECK } else { &icons::COPY })
                .variant(IconButtonVariant::GhostMuted)
                .control_size(ControlSize::Small)
                .label(if copied { "Copied" } else { "Copy" })
                .help("Copy the answer")
                .on_press_tx(move |s: &mut State, tx: &mut Tx| s.copy_answer(&id, tx))
                .build(k),
        );
    }
    if let Some(counts) = page
        .before_changes
        .iter()
        .chain(&page.turn_changes)
        .find(|counts| counts.after == m.id)
    {
        out = out
            .child(rune::spacer())
            .child(super::changes::button(k, counts));
    }
    out
}

/// The work between two texts on one rail, under a summary of its tool
/// calls. It shows open if `default` until clicked; `live` is the streaming
/// message's last part, so thinking there shows as under way.
fn tool_group(
    page: &Existing,
    k: &Kit,
    key: &str,
    steps: &[Step],
    live: Option<usize>,
    default: bool,
) -> El {
    let open = is_open(page, key, default);
    let tools: Vec<ToolCall> = steps
        .iter()
        .filter_map(|step| match step {
            Step::Tool(tool) => Some(*tool),
            Step::Thought(..) => None,
        })
        .collect();
    let thinking = |i: usize| live == Some(i);
    let busy_now = steps.iter().any(|step| match step {
        Step::Tool(tool) => tool.state == ToolState::Running,
        Step::Thought(i, _) => thinking(*i),
    });
    let toggle = key.to_owned();
    // The chevron sits on the rail's trunk, the summary over the rows' icons.
    let mut head = row()
        .h(TOOL_HEADER_HEIGHT)
        .gap(RAIL_X)
        .pr(6.0)
        .center_y()
        .min_w(0.0)
        .child(chevron(k, open))
        .child(
            text(tool_summary(&tools))
                .font(TextStyle {
                    size: TOOL_TEXT,
                    ..k.ty.ui
                })
                .color(k.c.text_muted)
                .truncate()
                .min_w(0.0),
        );
    if busy_now {
        head = head.child(busy(11.0, k));
    }
    let head = k
        .interactive(head.radius(6.0).shrink())
        .on_press(move |s: &mut State| toggle_group(s, &toggle, open));
    let mut group = col()
        .key(format!("tools-{key}"))
        .w_full()
        .child(row().w_full().child(head));
    if open {
        let last = steps.len().saturating_sub(1);
        let rail = steps.iter().enumerate().map(|(n, step)| match step {
            Step::Tool(tool) => tool_step(page, k, tool, n == last),
            Step::Thought(i, text) => {
                let key = format!("{key}:{i}");
                let open = is_open(page, &key, false);
                thought_row(k, &key, text, thinking(*i), open, n == last)
            }
        });
        group = group.child(col().w_full().ml(RAIL_X).children(rail));
    }
    group
}

/// Nest actual calls under their wrapper, expanded independently of the outer work.
fn tool_step(page: &Existing, k: &Kit, tool: &ToolCall, last: bool) -> El {
    if tool.calls.is_empty() && tool.name != "codemode" {
        return tool_row(k, tool, last);
    }
    let key = format!("{}#calls", tool.id);
    let open = is_open(page, &key, true);
    let tint = if tool.state == ToolState::Failed {
        tones(k).failed
    } else {
        k.c.text_muted
    };
    let lead = if tool.state == ToolState::Running {
        busy(12.0, k)
    } else {
        icon(icons::tool(tool.name), 14.0).color(tint)
    };
    let subject = row().center_y().gap(6.0).child(chevron(k, open)).child(
        text(format!("{} calls", tool.calls.len()))
            .font(rail_text(k))
            .color(tint),
    );
    let title = if tool.name == "codemode" {
        "Codemode"
    } else {
        tool.name
    };
    let head = k
        .interactive(rail_row(k, last && !open, lead, title, tint, subject))
        .on_press(move |s: &mut State| toggle_group(s, &key, open));
    let mut out = col().w_full().child(head);
    if open {
        let mut children = col().w_full().pl(RAIL_TEXT_X).border_sides(
            [0.0, 0.0, 0.0, if last { 0.0 } else { 1.0 }],
            k.c.border_muted,
        );
        for call in tool.calls {
            let mut line = row().w_full().center_y().child(
                tool_row(k, &ToolCall::from(call), false)
                    .min_w(0.0)
                    .grow(1.0),
            );
            if let Some(ms) = call.duration_ms {
                line = line.child(
                    text(format!("{:.2}s", ms / 1000.0))
                        .font(rail_text(k))
                        .color(k.c.text_faint)
                        .pr(8.0),
                );
            }
            children = children.child(line);
            let detail = call.error.clone().or_else(|| {
                (call.state == ToolState::Cancelled).then(|| "Cancelled or unfinished".to_owned())
            });
            if let Some(detail) = detail {
                children = children.child(
                    text(detail)
                        .font(rail_text(k))
                        .color(if call.error.is_some() {
                            tones(k).failed
                        } else {
                            k.c.text_faint
                        })
                        .wrap()
                        .selectable(true)
                        .pl(RAIL_TEXT_X)
                        .border_sides([0.0, 0.0, 0.0, 1.0], k.c.border_muted),
                );
            }
        }
        let call = tool.id.to_owned();
        let opened = page.tool_outputs.contains_key(tool.id);
        let label = if tool.name == "codemode" {
            "Script & combined output"
        } else {
            "Arguments & output"
        };
        let details = rail_row(k, true, chevron(k, opened), label, k.c.text_faint, el());
        children = children.child(
            k.interactive(details)
                .on_press(move |s: &mut State| s.toggle_tool_output(&call)),
        );
        if let Some(output) = page.tool_outputs.get(tool.id) {
            let body = match output {
                None => busy(12.0, k),
                Some(Err(error)) => text(error.clone())
                    .font(rail_text(k))
                    .color(tones(k).failed)
                    .wrap(),
                Some(Ok(output)) => {
                    let args = output.args["code"]
                        .as_str()
                        .map(str::to_owned)
                        .unwrap_or_else(|| {
                            serde_json::to_string_pretty(&output.args).unwrap_or_default()
                        });
                    let mut body = col()
                        .w_full()
                        .gap(8.0)
                        .child(
                            text(args)
                                .font(rail_text(k))
                                .color(k.c.text_faint)
                                .wrap()
                                .selectable(true),
                        )
                        .child(
                            text(output.text.clone())
                                .font(rail_text(k))
                                .color(k.c.text_muted)
                                .wrap()
                                .selectable(true),
                        );
                    if output.truncated {
                        body = body.child(
                            text("Output truncated")
                                .font(rail_text(k))
                                .color(k.c.text_faint),
                        );
                    }
                    if tool.state == ToolState::Running {
                        body = body.child(
                            text("Live snapshot. Close and reopen to refresh.")
                                .font(rail_text(k))
                                .color(k.c.text_faint),
                        );
                    }
                    body
                }
            };
            children = children.child(col().w_full().pl(RAIL_TEXT_X).pb(8.0).child(body));
        }
        out = out.child(children);
    }
    out
}

/// Where the rail's trunk runs, from the group's left edge.
const RAIL_X: f32 = 8.0;
/// The desktop's activity rows: one line each, branching off the trunk.
const TOOL_ROW_HEIGHT: f32 = 28.0;
const TOOL_HEADER_HEIGHT: f32 = 24.0;
const TOOL_TEXT: f32 = 12.0;

/// One call on the rail, as the desktop draws it: an elbow off the trunk,
/// the kind's icon, its verb, and its subject on one truncated line. File
/// tools show the file's name on a badge instead of its path.
fn tool_row(k: &Kit, tool: &ToolCall, last: bool) -> El {
    let failed = tool.state == ToolState::Failed;
    let tint = if failed {
        tones(k).failed
    } else {
        k.c.text_muted
    };
    let lead = match tool.state {
        ToolState::Running => busy(12.0, k),
        ToolState::Done => icon(icons::tool(tool.name), 14.0).color(tones(k).done),
        ToolState::Cancelled => icon(&icons::STOP, 14.0).color(k.c.text_faint),
        ToolState::Failed => icon(icons::tool(tool.name), 14.0).color(tint),
    };
    let subject = if tool.on_file() {
        file_badge(k, tool.subject, failed)
    } else {
        text(single_line(&tool.arg()))
            .font(rail_text(k))
            .color(tint)
            .truncate()
            .min_w(0.0)
    };
    rail_row(k, last, lead, tool.verb(), tint, subject)
}

/// A step of thinking on the rail: its first line, which is often a
/// heading, and the whole text below once clicked.
fn thought_row(k: &Kit, key: &str, text_in: &str, live: bool, open: bool, last: bool) -> El {
    let tint = k.c.text_muted;
    let lead = if live {
        busy(12.0, k)
    } else {
        icon(&icons::THOUGHT, 14.0).color(tint)
    };
    let first = text_in.lines().map(str::trim).find(|l| !l.is_empty());
    let subject = text(first.unwrap_or_default().replace("**", ""))
        .font(rail_text(k))
        .color(k.c.text_faint)
        .truncate()
        .min_w(0.0);
    let verb = if live { "Thinking" } else { "Thought" };
    let toggle = key.to_owned();
    let line = k
        .interactive(rail_row(k, last && !open, lead, verb, tint, subject))
        .on_press(move |s: &mut State| toggle_group(s, &toggle, open));
    if !open {
        return line;
    }
    // The trunk runs on beside the text, unless this is the rail's last step.
    let trunk = if last { 0.0 } else { 1.0 };
    let body = col()
        .w_full()
        .pl(RAIL_TEXT_X)
        .pb(6.0)
        .border_sides([0.0, 0.0, 0.0, trunk], k.c.border_muted)
        .child(
            text(text_in.trim().to_owned())
                .font(rail_text(k))
                .color(k.c.text_faint)
                .wrap()
                .selectable(true),
        );
    col().w_full().child(line).child(body)
}

/// Where a rail row's verb starts: past the elbow and the icon.
const RAIL_TEXT_X: f32 = 12.0 + 4.0 + 14.0 + 8.0;

fn rail_text(k: &Kit) -> TextStyle {
    TextStyle {
        size: TOOL_TEXT,
        line_height: 18.0,
        ..k.ty.ui
    }
}

/// A row on the rail: an elbow off the trunk, `lead`, `verb`, and `subject`
/// on one line.
fn rail_row(k: &Kit, last: bool, lead: El, verb: &str, tint: Color, subject: El) -> El {
    let elbow = rune::el()
        .w(12.0)
        .h(TOOL_ROW_HEIGHT / 2.0)
        .border_sides([0.0, 0.0, 1.0, 1.0], k.c.border_muted)
        .corners([0.0, 0.0, 0.0, 6.0]);
    // The trunk runs on past every row but the last, which ends in its elbow.
    let mut branch = col().w(12.0).h(TOOL_ROW_HEIGHT).child(elbow);
    if !last {
        branch = branch.border_sides([0.0, 0.0, 0.0, 1.0], k.c.border_muted);
    }
    row()
        .w_full()
        .h(TOOL_ROW_HEIGHT)
        .center_y()
        .child(branch)
        .child(rune::el().w(14.0).center().child(lead).ml(4.0).mr(8.0))
        .child(text(verb.to_owned()).font(rail_text(k)).color(tint).mr(8.0))
        .child(subject.shrink())
}

/// The file a call touched, by name, on the desktop's soft badge with the
/// file type's colour icon.
fn file_badge(k: &Kit, path: &str, failed: bool) -> El {
    let name = path
        .rsplit(['/', '\\'])
        .find(|s| !s.is_empty())
        .unwrap_or(path);
    let well = if k.dark { Color::BLACK } else { Color::WHITE }.alpha(0.16);
    row()
        .h(22.0)
        .gap(6.0)
        .pl(1.0)
        .pr(6.0)
        .radius(5.0)
        .center_y()
        .min_w(0.0)
        .fill(k.c.text_base.alpha(0.06))
        .child(
            rune::el()
                .size(20.0, 20.0)
                .radius(4.0)
                .fill(well)
                .center()
                .child(icon(file_icons::file(path), 14.0)),
        )
        .child(
            text(name.to_owned())
                .font(TextStyle {
                    size: TOOL_TEXT,
                    ..k.ty.ui
                })
                .color(if failed {
                    tones(k).failed
                } else {
                    k.c.text_base.alpha(0.85)
                })
                .truncate()
                .min_w(0.0),
        )
}

/// `text` on one line, its runs of whitespace (newlines too) one space each.
fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// The dialog pi waits on, as its kind asks: pick an option, confirm, or
/// type an answer.
fn question(k: &Kit, form: &QuestionForm) -> El {
    let t = tones(k);
    let d = &form.dialog;
    let mut card = col()
        .w_full()
        .gap(10.0)
        .padding(12.0)
        .radius(12.0)
        .fill(k.c.layer_02)
        .border(1.0, k.c.border_base)
        .child(
            row().gap(6.0).center_y().child(dot(t.input)).child(
                text(d.title.clone())
                    .font(k.ty.ui_medium)
                    .color(k.c.text_base)
                    .wrap()
                    .grow(1.0)
                    .min_w(0.0),
            ),
        );
    if let Some(message) = d.message.as_ref().filter(|m| !m.trim().is_empty()) {
        card = card.child(
            text(message.clone())
                .font(k.ty.ui)
                .color(k.c.text_muted)
                .wrap()
                .selectable(true),
        );
    }
    match d.dialog {
        DialogKind::Select => {
            for (i, option) in d.options.iter().enumerate() {
                let value = option.clone();
                let opt = row()
                    .w_full()
                    .px(8.0)
                    .py(6.0)
                    .radius(8.0)
                    .center_y()
                    .fill(k.c.layer_01)
                    .child(
                        text(option.clone())
                            .font(k.ty.ui)
                            .color(k.c.text_base)
                            .wrap()
                            .grow(1.0)
                            .min_w(0.0),
                    );
                card = card.child(
                    k.interactive(opt)
                        .key(format!("opt-{i}"))
                        .on_press(move |s: &mut State| s.respond(Answer::Value(value.clone()))),
                );
            }
        }
        DialogKind::Confirm => {
            let button = |label: &'static str, yes: bool, variant| {
                Button::new(label)
                    .variant(variant)
                    .disabled(form.sending)
                    .on_press(move |s: &mut State| s.respond(Answer::Confirmed(yes)))
                    .build(k)
            };
            card = card.child(
                row()
                    .gap(8.0)
                    .justify_end()
                    .child(button("No", false, ButtonVariant::Outline))
                    .child(button("Yes", true, ButtonVariant::Contrast)),
            );
        }
        DialogKind::Input | DialogKind::Editor => {
            let field = if d.dialog == DialogKind::Editor {
                TextArea::new(&form.field)
                    .placeholder("Your answer")
                    .auto_grow(3, 12)
                    .on_edit(State::answer_field)
                    .on_submit(|s: &mut State, _: &mut Tx| s.respond_typed())
                    .build(k)
            } else {
                TextInput::new(&form.field)
                    .placeholder("Your answer")
                    .on_edit(State::answer_field)
                    .on_submit(|s: &mut State, _: &mut Tx| s.respond_typed())
                    .build(k)
            };
            card = card.child(field.key("answer")).child(
                row().justify_end().child(
                    Button::new("Send")
                        .variant(ButtonVariant::Contrast)
                        .disabled(form.sending)
                        .on_press(State::respond_typed)
                        .build(k),
                ),
            );
        }
    }
    card
}
