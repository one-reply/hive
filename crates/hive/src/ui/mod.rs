//! GPUI views. Each view observes only the state entity it renders.

pub(crate) mod agent_logo;
pub mod app_menu;
pub(crate) mod diff_panel;
pub(crate) mod editor_panel;
pub(crate) mod files_panel;
mod git_controls;
pub(crate) mod git_panel;
pub(crate) mod hover;
pub(crate) mod key_hint;
pub mod keymap;
pub(crate) mod project_badge;
pub(crate) mod release_notes;
pub mod settings;
pub mod sidebar;
pub mod status_bar;
pub(crate) mod syntax;
pub(crate) mod text_field;
pub mod theme;
mod update_available;
pub mod workspace;

pub(crate) mod add_project;
