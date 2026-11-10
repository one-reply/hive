//! GPUI entities that own application state. Blocking work goes to the background
//! executor inside these; views never touch `hive_core` IO directly.

pub(crate) mod agents;
pub(crate) mod ai_providers;
pub(crate) mod documents;
pub(crate) mod files;
pub(crate) mod git;
pub(crate) mod git_conflict;
pub(crate) mod git_diff;
pub(crate) mod notifications;
pub mod projects;
pub mod store;
pub(crate) mod terminal_session;
pub(crate) mod terminals;
pub(crate) mod updates;
pub mod usage;

pub(crate) mod pull_requests;

pub(crate) mod remote_project;

pub(crate) mod file_io;
