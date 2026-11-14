//! Waiting prompts, kept apart from pi's transcript until their turn starts.

use anyhow::{Context, Result, bail};
use serde::Serialize;

use super::{Chat, Engine};
use crate::skills;
use crate::transcript::Image;

/// Bound unsent drafts, including base64 images, so a queue cannot grow without limit.
const MAX_BYTES: usize = 12 * 1024 * 1024;
/// Keep both the view and the work waiting behind one chat small.
const MAX_MESSAGES: usize = 32;

/// A waiting message's view, without copying image data into every feed frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Queued {
    pub id: String,
    pub text: String,
    pub images: usize,
    pub error: Option<String>,
}

/// An unsent draft; it is not chat history and lives only for this engine's life.
#[derive(Debug, Clone)]
pub(super) struct Waiting {
    pub id: String,
    pub text: String,
    pub images: Vec<Image>,
    pub error: Option<String>,
}

impl Waiting {
    /// Count the bytes retained by this draft.
    fn bytes(&self) -> usize {
        self.text.len()
            + self
                .images
                .iter()
                .map(|image| image.data.len())
                .sum::<usize>()
    }

    /// Describe the draft without its image payloads.
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
    /// Describe the chat's waiting messages while holding its state lock.
    pub(super) fn queued(chat: Option<&Chat>) -> Vec<Queued> {
        chat.into_iter()
            .flat_map(|chat| &chat.waiting)
            .map(Waiting::view)
            .collect()
    }

    /// Hold a prompt until the agent finishes, without handing it to pi yet.
    pub(super) fn enqueue(&self, id: &str, text: &str, images: Vec<Image>) -> Result<()> {
        let draft = Waiting {
            id: uuid::Uuid::new_v4().to_string(),
            text: text.to_owned(),
            images,
            error: None,
        };
        let mut state = self.state();
        let chat = state.chats.get_mut(id).context("the chat is gone")?;
        let bytes = chat.waiting.iter().map(Waiting::bytes).sum::<usize>() + draft.bytes();
        if chat.waiting.len() >= MAX_MESSAGES || bytes > MAX_BYTES {
            bail!("the message queue is full (32 messages or 12 MiB)");
        }
        chat.waiting.push_back(draft);
        chat.changed();
        Ok(())
    }

    /// Edit a message that has not started, preserving its images and place.
    ///
    /// # Errors
    ///
    /// Fail if it has already started, the edit is empty, or the queue is full.
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

    /// Delete one message that has not started.
    ///
    /// # Errors
    ///
    /// Fail if it has already started or was removed.
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

    /// Start the oldest waiting prompt if the chat is idle.
    ///
    /// # Errors
    ///
    /// Fail if the agent is still working or pi rejects the prompt. Keep rejected drafts for retry.
    pub async fn resume_queue(&self, id: &str) -> Result<()> {
        let stops = self.stops(id);
        let ops = self.ops(id)?;
        let _ops = ops.lock().await;
        self.refuse_elsewhere(id)?;
        loop {
            let draft = {
                let state = self.state();
                if state.stopping || state.chats.get(id).is_none_or(|chat| chat.stops != stops) {
                    return Ok(());
                }
                let chat = state.chats.get(id).context("the chat is gone")?;
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

    /// Dispatch after a clean settle, outside the event reader and under the chat's command lock.
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
