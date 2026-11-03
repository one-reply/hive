//! Embedded terminal: the entity ([`view::TerminalView`]), its painter
//! ([`element::TerminalElement`]), key mapping ([`keys`]) and the dock panel ([`panel`]).

pub mod element;
pub mod keys;
pub mod panel;
pub mod view;

pub use panel::TerminalPanel;
