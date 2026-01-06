//! Progress indicators for CLI operations.
//!
//! Provides user feedback during long-running operations like evaluation and building.
//! Mimics the nix CLI's progress display style.

use indicatif::{ProgressBar, ProgressStyle};
use std::io::IsTerminal;
use std::time::Duration;

/// Check if stderr is a terminal (for deciding whether to show spinners)
pub fn is_interactive() -> bool {
    std::io::stderr().is_terminal()
}

/// A status indicator that shows a spinner with a message.
/// Automatically hides when dropped.
pub struct Status {
    bar: Option<ProgressBar>,
}

impl Status {
    /// Create a new status indicator with a spinner.
    /// If not running in a terminal, returns a no-op status.
    pub fn new(message: &str) -> Self {
        if !is_interactive() {
            // Not a terminal, just print the message
            eprintln!("{}", message);
            return Self { bar: None };
        }

        let bar = ProgressBar::new_spinner();
        bar.set_style(
            ProgressStyle::default_spinner()
                .template("{spinner:.cyan} {msg}")
                .expect("valid template")
                .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
        );
        bar.set_message(message.to_string());
        bar.enable_steady_tick(Duration::from_millis(80));

        Self { bar: Some(bar) }
    }

    /// Update the status message.
    pub fn set_message(&self, message: &str) {
        if let Some(ref bar) = self.bar {
            bar.set_message(message.to_string());
        }
    }

    /// Finish with a success message.
    pub fn finish(&self, message: &str) {
        if let Some(ref bar) = self.bar {
            bar.finish_with_message(message.to_string());
        } else {
            eprintln!("{}", message);
        }
    }

    /// Finish and clear the line (no message).
    pub fn finish_and_clear(&self) {
        if let Some(ref bar) = self.bar {
            bar.finish_and_clear();
        }
    }
}

impl Drop for Status {
    fn drop(&mut self) {
        if let Some(ref bar) = self.bar {
            if !bar.is_finished() {
                bar.finish_and_clear();
            }
        }
    }
}

// Convenience functions for common operations

/// Show "evaluating..." status
pub fn evaluating(target: &str) -> Status {
    Status::new(&format!("evaluating '{}'...", target))
}

/// Show "building..." status
pub fn building(target: &str) -> Status {
    Status::new(&format!("building '{}'...", target))
}

/// Show "copying..." status
pub fn copying(target: &str) -> Status {
    Status::new(&format!("copying '{}'...", target))
}

/// Show "fetching..." status
pub fn fetching(target: &str) -> Status {
    Status::new(&format!("fetching '{}'...", target))
}

/// Show a generic status
pub fn status(message: &str) -> Status {
    Status::new(message)
}
