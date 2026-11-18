use super::{State, Window};
use crate::wsl::{self, Status};

impl State {
    pub fn check_wsl(&mut self) {
        if self.window != Window::Main || matches!(self.wsl_setup, Some(Status::Installing)) {
            return;
        }
        self.wsl_setup = Some(Status::Checking);
        self.api.run(
            |_| async { wsl::inspect().await },
            |s, result, _| s.wsl_result(result),
        );
    }

    fn wsl_result(&mut self, result: anyhow::Result<Status>) {
        self.wsl_setup = Some(result.unwrap_or_else(|e| Status::Failed(format!("{e:#}"))));
        if let Some(Status::NeedsEngine(distribution)) = &self.wsl_setup
            && self.prefs.borrow().managed_wsl.as_ref() == Some(distribution)
        {
            self.setup_engine();
        }
    }

    pub fn setup_engine(&mut self) {
        let Some(Status::NeedsEngine(distribution)) = &self.wsl_setup else {
            return;
        };
        let distribution = distribution.clone();
        self.prefs.borrow_mut().managed_wsl = Some(distribution.clone());
        self.save_prefs();
        self.wsl_setup = Some(Status::Installing);
        self.api.run(
            move |_| async move { wsl::install(&distribution).await },
            |s, result, _| {
                // Don't automatically loop if the default distribution changed during setup.
                s.wsl_setup = Some(result.unwrap_or_else(|e| Status::Failed(format!("{e:#}"))));
            },
        );
    }

    pub fn install_wsl(&mut self) {
        if let Err(error) = wsl::install_wsl() {
            self.wsl_setup = Some(Status::Failed(format!("{error:#}")));
        }
    }

    pub fn open_wsl_terminal(&mut self) {
        if let Err(error) = wsl::open_terminal() {
            self.wsl_setup = Some(Status::Failed(format!("{error:#}")));
        }
    }
}
