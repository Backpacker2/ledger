//! Running a prompt with Claude Code: one click in the app starts the `claude` command-line program,
//! non-interactively, in the prompt's project folder, and the deliverable it writes lands where the app expects it.
//!
//! * `config`: the settings (program path, permission mode, model, guard rails), in `.ledger/claude-code.json`.
//! * `cli`: finding the program, the command line, the guard rails' settings file.
//! * `prompt`: what is sent, and checking that the prompt and its project are there.
//! * `runner`: the traits that start a process, and the real implementation (no shell, no console window on
//!   Windows, the whole process tree stopped on cancel).
//! * `runs`: the runs themselves, with their output, cancel, and tidying the vault afterwards.
//! * `versions`: which deliverables a run changed, and what is published and linked to the prompt for them.
//! * `render`, `redact`, `records`, `finish`: showing the output, keeping secrets out of it, the record of a run
//!   that is kept in its thread as a note, and the vault reload.
//!
//! `desktop` holds the Tauri side (desktop app only): the commands, and the host that tells the window what is happening. The runs live
//! in a state of their own that is set up the first time it is needed, so the app's own setup is untouched.

pub mod cli;
pub mod config;
pub mod finish;
pub mod prompt;
pub mod records;
pub mod redact;
pub mod render;
pub mod runner;
pub mod runs;
pub mod versions;

#[cfg(feature = "desktop")]
mod desktop;
#[cfg(feature = "desktop")]
pub use desktop::*;
