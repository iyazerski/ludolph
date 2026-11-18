use rune::{El, col, row, text};
use rune_kit::{Button, ButtonVariant, Kit};

use super::widgets::busy;
use crate::app::State;
use crate::wsl::Status;

pub fn view(status: &Status, k: &Kit) -> El {
    let (title, description) = match status {
        Status::Checking => ("Checking WSL", "Looking for Ludolph's engine in your default Linux distribution.".to_owned()),
        Status::NeedsWsl(detail) => ("Set up Linux for pi", format!("Ludolph runs pi and its tools in WSL. Install WSL, restart Windows if asked, then open your Linux distribution to finish creating its user. Come back here and check again.\n\n{detail}")),
        Status::NeedsEngine(distribution) => ("Set up Ludolph's engine", format!("Install the bundled engine in {distribution}? Ludolph will also keep it in sync when you update the Windows app. This stops any running Ludolph chats in that distribution. Your projects, pi settings and chat history stay where they are.")),
        Status::NeedsPi(distribution) => ("Install pi in WSL", format!("Ludolph's engine is installed in {distribution}. Install pi there using the instructions at pi.dev, then check again. You can sign in to a provider from Settings > Accounts once Ludolph connects.")),
        Status::Installing => ("Setting up the engine", "Installing the engine bundled with this version of Ludolph. Waiting for any old engine to stop first.".to_owned()),
        Status::Failed(error) => ("Setup couldn't finish", error.clone()),
        Status::Ready => unreachable!("ready setup isn't shown"),
    };
    let button = |label: &str, action: fn(&mut State)| {
        Button::new(label)
            .variant(ButtonVariant::Neutral)
            .on_press(action)
            .build(k)
    };
    let mut actions = row().gap(10.0);
    match status {
        Status::NeedsWsl(_) => {
            actions = actions
                .child(button("Install WSL", State::install_wsl))
                .child(button("Check again", State::check_wsl));
        }
        Status::NeedsEngine(_) => {
            actions = actions.child(button("Install engine", State::setup_engine))
        }
        Status::NeedsPi(_) => {
            actions = actions
                .child(
                    Button::new("pi install instructions")
                        .variant(ButtonVariant::Neutral)
                        .on_press_tx(|s: &mut State, tx| s.open_link("https://pi.dev", tx))
                        .build(k),
                )
                .child(button("Open WSL terminal", State::open_wsl_terminal))
                .child(button("Check again", State::check_wsl));
        }
        Status::Failed(_) => actions = actions.child(button("Try again", State::check_wsl)),
        Status::Checking | Status::Installing => actions = actions.child(busy(18.0, k)),
        Status::Ready => {}
    }
    col()
        .w_full()
        .h_full()
        .justify_center()
        .items_center()
        .px(32.0)
        .py(32.0)
        .fill(k.c.bg_base)
        .child(
            col()
                .w_full()
                .max_w(620.0)
                .gap(18.0)
                .child(text(title).font(k.ty.ui_medium).color(k.c.text_base).wrap())
                .child(text(description).font(k.ty.ui).color(k.c.text_muted).wrap())
                .child(actions)
                .child(
                    Button::new("Settings")
                        .variant(ButtonVariant::Outline)
                        .on_press(State::open_settings)
                        .build(k),
                ),
        )
}
