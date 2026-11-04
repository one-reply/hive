//! Hive domain and adapters. No UI framework in here: everything is plain Rust,
//! `serde`-serializable, and runtime-agnostic (blocking fns the caller runs on a
//! background executor). This is what a future backend will share as its proto.

pub mod agent;
pub mod commit_message;
pub mod git;
pub mod host;
pub mod identity;
pub mod keychain;
pub mod llm;
pub mod projects;
pub mod settings;
pub mod state;
pub mod terminal;
pub mod usage;
pub mod worktree;
