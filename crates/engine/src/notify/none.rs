use anyhow::Result;

/// Nothing posts `notify(3)` names outside macOS, so no `Changes` can exist.
#[derive(Debug)]
pub enum Changes {}

impl Changes {
    pub async fn next(&mut self) -> Result<()> {
        match *self {}
    }
}
