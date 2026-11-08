//! Which launchable agent CLIs this Mac has, for menus that offer to start one.
use super::Agents;
use gpui_kit::*;
use hive_core::agent::{
    AgentKind,
    launch::{AGENTS, Launch},
};

impl Agents {
    /// Launchable agents found at the last scan, in `AGENTS` order.
    pub(crate) fn installed(&self) -> &[AgentKind] {
        &self.installed
    }

    /// Rescans in the background. A menu reads the cached list when it opens, so it never waits
    /// on the disk; a CLI installed meanwhile shows up the next time.
    pub(crate) fn refresh_installed(&mut self, cx: &mut Context<Self>) {
        self._installed_scan = cx.spawn(async move |this, cx| {
            let installed = cx
                .background_executor()
                .spawn(async {
                    AGENTS
                        .into_iter()
                        .filter(|agent| Launch::New(*agent).executable().is_ok())
                        .collect()
                })
                .await;
            // Nothing renders the list, so no notify: menus read it when they open.
            let _ = this.update(cx, |this, _| this.installed = installed);
        });
    }
}
