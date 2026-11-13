//! A chat's live feed for one subscriber: its latest messages once, then
//! only the ones that changed, so a window shows a reply as it streams
//! without reading the page again.

use std::collections::HashMap;

use anyhow::Result;
use serde_json::{Value, json};

use crate::engine::{Engine, Status};
use crate::tools::{pending_input, summary, turn_changes_of};
use crate::transcript::{LATEST, Message, Part, Window};

/// Follows one chat, keeping what its subscriber was last sent.
pub struct Feed {
    engine: Engine,
    chat: String,
    thinking: bool,
    /// Nothing sent yet.
    first: bool,
    /// The page's first message, once pi has written it. The page runs from
    /// it, so new messages don't push out ones the subscriber has.
    anchor: Option<String>,
    sent: HashMap<String, Message>,
    /// The run changes, with the messages and status they were found for:
    /// runs are saved as they settle, which changes one or the other.
    changes: Option<(Vec<String>, Status, Vec<Value>)>,
}

impl Feed {
    /// Follow chat `chat`, with the model's thinking if `thinking`.
    pub fn new(engine: Engine, chat: String, thinking: bool) -> Self {
        Self {
            engine,
            chat,
            thinking,
            first: true,
            anchor: None,
            sent: HashMap::new(),
            changes: None,
        }
    }

    /// The next frame. `ids` is the page's messages in order, and
    /// `messages` those that are new or changed since the last frame. With
    /// `reset`, the subscriber drops what it has first: on the first frame,
    /// and when the page it had is gone, as after `/tree`. The chat, its
    /// run, the question it asks and its run changes come whole.
    ///
    /// # Errors
    ///
    /// Fails if the chat is gone, or its session file can't be read.
    pub async fn frame(&mut self) -> Result<Value> {
        let window = self
            .anchor
            .clone()
            .map_or(Window::Latest(LATEST), |id| Window::From { id, earlier: 0 });
        let mut view = self.engine.view(&self.chat, window).await?;
        let mut reset = std::mem::take(&mut self.first);
        if let Some(anchor) = &self.anchor
            && view.page.messages.first().map(|m| &m.id) != Some(anchor)
        {
            view = self.engine.view(&self.chat, Window::Latest(LATEST)).await?;
            reset = true;
        }
        if reset {
            self.sent.clear();
        }
        let messages = std::mem::take(&mut view.page.messages);
        let messages: Vec<Message> = messages
            .into_iter()
            .map(|mut message| {
                if !self.thinking {
                    message
                        .parts
                        .retain(|part| !matches!(part, Part::Thinking { .. }));
                }
                message
            })
            .collect();
        self.anchor = messages
            .first()
            .filter(|m| m.written())
            .map(|m| m.id.clone());
        let ids: Vec<String> = messages.iter().map(|m| m.id.clone()).collect();
        let changed: Vec<&Message> = messages
            .iter()
            .filter(|m| self.sent.get(&m.id) != Some(*m))
            .collect();
        let turn_changes = self.turn_changes(&messages, &ids, view.chat.status).await;
        let frame = json!({
            "reset": reset,
            "chat": summary(&view.chat, self.engine.config()),
            "run": view.run,
            "pendingInput": view.question.as_ref().map(pending_input),
            "turnChanges": turn_changes,
            "total": view.page.total,
            "olderRemaining": view.page.older(),
            "ids": ids,
            "messages": changed,
        });
        self.sent = messages.into_iter().map(|m| (m.id.clone(), m)).collect();
        Ok(frame)
    }

    /// The saved runs among `messages` that changed files, read again only
    /// when the messages or the status changed.
    async fn turn_changes(
        &mut self,
        messages: &[Message],
        ids: &[String],
        status: Status,
    ) -> Vec<Value> {
        if let Some((for_ids, for_status, changes)) = &self.changes
            && for_ids == ids
            && *for_status == status
        {
            return changes.clone();
        }
        let changes = match self.engine.run_counts(&self.chat).await {
            Ok(runs) => turn_changes_of(messages, &runs),
            // The chat still shows; it only loses its changes buttons.
            Err(e) => {
                eprintln!("reading chat {}'s run changes: {e:#}", self.chat);
                Vec::new()
            }
        };
        self.changes = Some((ids.to_vec(), status, changes.clone()));
        changes
    }
}
