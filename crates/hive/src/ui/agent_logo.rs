//! One logo per `AgentKind`, shared by the sidebar agents strip, the new-worktree
//! dialog, the Git settings picker and the default CLI agent picker.
//! Vendors without a monochrome mark get a generic terminal glyph: a wrong logo is worse than none.
use gpui_kit::{component::ActiveTheme as _, *};
use hive_core::agent::AgentKind;

/// Anthropic's Claude orange; every other vendor uses the foreground color like the status bar.
const CLAUDE_COLOR: u32 = 0xd97757;

pub(crate) fn svg(kind: AgentKind) -> &'static [u8] {
    match kind {
        AgentKind::ClaudeCode => include_bytes!("../../assets/claude.svg"),
        AgentKind::Codex => include_bytes!("../../assets/openai.svg"),
        AgentKind::OpenCode => include_bytes!("../../assets/opencode.svg"),
        AgentKind::GeminiCli => include_bytes!("../../assets/gemini.svg"),
        AgentKind::CursorAgent => include_bytes!("../../assets/cursor.svg"),
        AgentKind::CopilotCli => include_bytes!("../../assets/copilot.svg"),
        AgentKind::QwenCode => include_bytes!("../../assets/qwen.svg"),
        AgentKind::Antigravity => include_bytes!("../../assets/antigravity.svg"),
        AgentKind::Amp | AgentKind::Aider | AgentKind::Droid => {
            include_bytes!("../../assets/agent-terminal.svg")
        }
    }
}

pub(crate) fn color(kind: AgentKind, cx: &App) -> Hsla {
    match kind {
        AgentKind::ClaudeCode => rgb(CLAUDE_COLOR).into(),
        _ => cx.theme().sidebar_foreground,
    }
}
