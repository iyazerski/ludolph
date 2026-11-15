use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::engine::{Engine, Status};
use crate::tools::{pending_input, summary, turn_changes_of};
use crate::transcript::{LATEST, Message, Part, Window};

pub async fn follow<F: Future<Output = bool>>(
    mut changes: watch::Receiver<u64>,
    mut feed: Feed,
    every: Duration,
    mut send: impl FnMut(Value) -> F,
) {
    loop {
        changes.mark_unchanged();
        let frame = match feed.frame().await {
            Ok(frame) => frame,
            Err(e) => {
                send(json!({"error": format!("{e:#}")})).await;
                return;
            }
        };
        if !send(frame).await {
            return;
        }
        tokio::time::sleep(every).await;
        if changes.changed().await.is_err() {
            return;
        }
    }
}

pub struct Feed {
    engine: Engine,
    chat: String,
    thinking: bool,
    first: bool,
    /// Anchor on a persisted message so new messages do not evict the visible page.
    anchor: Option<String>,
    sent: HashMap<String, Message>,
    changes: Option<(Vec<String>, Status, u64, Vec<Value>)>,
}

impl Feed {
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

    /// Send ordered `ids` with changed messages only; `reset` discards the client's old page.
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
        let older = view.page.older();
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
        let turn_changes = self
            .turn_changes(
                &messages,
                &ids,
                view.chat.status,
                view.latest_turn,
                view.turns_revision,
            )
            .await;
        let frame = json!({
            "reset": reset,
            "chat": summary(&view.chat, self.engine.config()),
            "run": view.run,
            "failure": view.failure,
            "queued": view.queued,
            "pendingInput": view.question.as_ref().map(pending_input),
            "turnChanges": turn_changes,
            "total": view.page.total,
            "olderRemaining": older,
            "ids": ids,
            "messages": changed,
        });
        self.sent = messages.into_iter().map(|m| (m.id.clone(), m)).collect();
        Ok(frame)
    }

    async fn turn_changes(
        &mut self,
        messages: &[Message],
        ids: &[String],
        status: Status,
        latest_turn: Option<i64>,
        revision: u64,
    ) -> Vec<Value> {
        if let Some((for_ids, for_status, for_revision, changes)) = &self.changes
            && for_ids == ids
            && *for_status == status
            && *for_revision == revision
        {
            return changes.clone();
        }
        let changes = match self.engine.run_counts(&self.chat).await {
            Ok(runs) => turn_changes_of(messages, &runs, latest_turn),
            Err(e) => {
                eprintln!("reading chat {}'s run changes: {e:#}", self.chat);
                Vec::new()
            }
        };
        self.changes = Some((ids.to_vec(), status, revision, changes.clone()));
        changes
    }
}
