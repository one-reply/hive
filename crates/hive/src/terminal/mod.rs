//! Embedded terminal: the entity ([`view::TerminalView`]), its painter
//! ([`element::TerminalElement`]), key mapping ([`keys`]), the dock panel ([`panel`]) and the
//! layout of its panes ([`pane_tree`]).

pub mod element;
pub mod keys;
mod mouse;
pub mod pane_tree;
pub mod panel;
pub mod view;

pub use panel::TerminalPanel;
