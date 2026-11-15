use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::{Chat, Engine};
use crate::attachments::Attachment;
use crate::skills;

/// Include image payloads in the queue's memory budget.
const MAX_BYTES: usize = 12 * 1024 * 1024;
const MAX_MESSAGES: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Queued {
    pub id: String,
    pub text: String,
    pub images: usize,
    pub error: Option<String>,
}

/// Unsent drafts are ephemeral, not session history.
#[derive(Debug, Clone)]
pub(super) struct Waiting {
    pub id: String,
    pub text: String,
    pub images: Vec<Attachment>,
    pub error: Option<String>,
}

impl Waiting {
    fn bytes(&self) -> usize {
        self.text.len() + self.images.iter().map(Attachment::bytes).sum::<usize>()
    }

    fn view(&self) -> Queued {
        Queued {
            id: self.id.clone(),
            text: skills::as_typed(&self.text).into_owned(),
            images: self.images.len(),
            error: self.error.clone(),
        }
    }
}

impl Engine {
    pub(super) fn queued(chat: Option<&Chat>) -> Vec<Queued> {
        chat.into_iter()
            .flat_map(|chat| &chat.waiting)
            .map(Waiting::view)
            .collect()
    }

    pub(super) fn enqueue(
        &self,
        id: &str,
        text: &str,
        images: Vec<Attachment>,
        stops: u64,
    ) -> Result<()> {
        let draft = Waiting {
            id: uuid::Uuid::new_v4().to_string(),
            text: text.to_owned(),
            images,
            error: None,
        };
        let mut state = self.state();
        let chat = state.chats.get_mut(id).context("the chat is gone")?;
        if chat.stops != stops {
            bail!("stopped before it was sent");
        }
        let bytes = chat.waiting.iter().map(Waiting::bytes).sum::<usize>() + draft.bytes();
        if chat.waiting.len() >= MAX_MESSAGES || bytes > MAX_BYTES {
            bail!("the message queue is full (32 messages or 12 MiB)");
        }
        chat.waiting.push_back(draft);
        chat.changed();
        Ok(())
    }

    pub async fn edit_queued(&self, id: &str, message: &str, text: &str) -> Result<()> {
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        self.refuse_elsewhere(id)?;
        let mut state = self.state();
        let chat = state.chats.get_mut(id).context("the chat is gone")?;
        let bytes = chat.waiting.iter().map(Waiting::bytes).sum::<usize>();
        let draft = chat
            .waiting
            .iter_mut()
            .find(|draft| draft.id == message)
            .context("this message has already started or was removed")?;
        let text = text.trim();
        if text.is_empty() && draft.images.is_empty() {
            bail!("the message is empty");
        }
        if bytes - draft.text.len() + text.len() > MAX_BYTES {
            bail!("the message queue is full (12 MiB)");
        }
        text.clone_into(&mut draft.text);
        draft.error = None;
        chat.changed();
        Ok(())
    }

    pub async fn delete_queued(&self, id: &str, message: &str) -> Result<()> {
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        let mut state = self.state();
        let chat = state.chats.get_mut(id).context("the chat is gone")?;
        let position = chat
            .waiting
            .iter()
            .position(|draft| draft.id == message)
            .context("this message has already started or was removed")?;
        chat.waiting.remove(position);
        chat.changed();
        Ok(())
    }

    /// Keep a rejected draft available for retry.
    pub async fn steer_queued(&self, id: &str, message: &str) -> Result<()> {
        let stops = self.stops(id);
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        self.refuse_elsewhere(id)?;
        let draft = {
            let state = self.state();
            state
                .chats
                .get(id)
                .and_then(|chat| chat.waiting.iter().find(|draft| draft.id == message))
                .cloned()
                .context("this message has already started or was removed")?
        };
        let result = self.send_locked(id, &draft.text, draft.images, stops).await;
        let mut state = self.state();
        if let Some(chat) = state.chats.get_mut(id) {
            if result.is_ok() {
                chat.waiting.retain(|waiting| waiting.id != message);
            } else if let Some(waiting) = chat
                .waiting
                .iter_mut()
                .find(|waiting| waiting.id == message)
            {
                waiting.error = result.as_ref().err().map(|error| format!("{error:#}"));
            }
            chat.changed();
        }
        result.map(|_| ())
    }

    /// Keep a rejected draft available for retry.
    pub async fn resume_queue(&self, id: &str) -> Result<()> {
        let stops = self.stops(id);
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        self.refuse_elsewhere(id)?;
        loop {
            let draft = {
                let state = self.state();
                if state.stopping {
                    return Ok(());
                }
                let Some(chat) = state.chats.get(id).filter(|chat| chat.stops == stops) else {
                    return Ok(());
                };
                if chat
                    .process
                    .as_ref()
                    .is_some_and(|running| running.live.working())
                {
                    bail!("the agent is still working");
                }
                chat.waiting.front().cloned()
            };
            let Some(draft) = draft else { return Ok(()) };
            let result = self.send_locked(id, &draft.text, draft.images, stops).await;
            let mut state = self.state();
            if let Some(chat) = state.chats.get_mut(id) {
                if result.is_ok() {
                    chat.waiting.retain(|waiting| waiting.id != draft.id);
                } else if let Some(waiting) = chat
                    .waiting
                    .front_mut()
                    .filter(|waiting| waiting.id == draft.id)
                {
                    waiting.error = result.as_ref().err().map(|error| format!("{error:#}"));
                }
                chat.changed();
            }
            let delivery = result?;
            if delivery != super::Delivery::Handled {
                return Ok(());
            }
        }
    }

    /// Run outside the event reader, under the chat's `ops` lock.
    pub(super) fn queue_soon(&self, id: &str) {
        if !self
            .state()
            .chats
            .get(id)
            .is_some_and(|chat| !chat.waiting.is_empty())
        {
            return;
        }
        let (engine, id) = (self.clone(), id.to_owned());
        tokio::spawn(async move {
            if let Err(error) = engine.resume_queue(&id).await {
                eprintln!("chat {id}: sending the next queued message: {error:#}");
            }
        });
    }
}
