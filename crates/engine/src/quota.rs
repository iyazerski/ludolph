//! Subscription quota that Ludolph's quota extension reports for the chat's provider.

use serde::{Deserialize, Serialize};

/// The `setStatus` key the extension reports under.
pub const STATUS_KEY: &str = "ludolph-quota";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub label: String,
    /// Share used, 0 to 1.
    pub used: f64,
    /// Epoch milliseconds.
    pub resets_at: Option<i64>,
    /// Dollars, for a budget.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spent: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<f64>,
}

#[derive(Debug, Deserialize)]
pub struct Report {
    pub provider: String,
    pub windows: Vec<Window>,
}

/// A window whose reset has passed counts as unused until the next report.
pub fn current(windows: &[Window], now_ms: i64) -> Vec<Window> {
    windows
        .iter()
        .cloned()
        .map(|mut window| {
            if window.resets_at.is_some_and(|at| at <= now_ms) {
                window.used = 0.0;
                window.spent = window.spent.map(|_| 0.0);
                window.resets_at = None;
            }
            window
        })
        .collect()
}
