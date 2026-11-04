//! GPUI entities that own application state. Blocking work goes to the background
//! executor inside these; views never touch `hive_core` IO directly.

pub(crate) mod agents;
pub(crate) mod ai_providers;
pub(crate) mod git;
pub(crate) mod git_diff;
pub mod projects;
pub mod store;
pub(crate) mod terminal_session;
pub(crate) mod terminals;
pub(crate) mod updates;
pub mod usage;
