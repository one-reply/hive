//! Hive domain and adapters. No UI framework in here: everything is plain Rust,
//! `serde`-serializable, and runtime-agnostic (blocking fns the caller runs on a
//! background executor). This is what a future backend will share as its proto.

#[cfg(feature = "desktop")]
pub mod agent;
pub mod code_navigation;
pub mod command;
#[cfg(feature = "desktop")]
pub mod commit_message;
pub mod files;
pub mod git;
pub mod host;
#[cfg(feature = "desktop")]
pub mod identity;
#[cfg(feature = "desktop")]
pub mod keychain;
#[cfg(feature = "desktop")]
pub mod llm;
#[cfg(feature = "desktop")]
pub mod projects;
#[cfg(feature = "desktop")]
pub mod release_notes;
#[cfg(feature = "desktop")]
pub mod settings;
#[cfg(feature = "desktop")]
pub mod state;
#[cfg(feature = "desktop")]
pub mod terminal;
#[cfg(feature = "desktop")]
pub mod usage;
#[cfg(feature = "desktop")]
pub mod worktree;

mod path_encoding;
pub mod remote;
