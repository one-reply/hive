//! Stage icons follow the recipe roles drawn on the template map.
use super::super::map::RoleKind;
use super::*;
pub(super) use hive_core::pipeline::{Stage, StageStatus};

pub(super) trait StageIcon {
    fn icon(self, run: &Run) -> Lucide;
}

impl StageIcon for Stage {
    fn icon(self, run: &Run) -> Lucide {
        match self {
            Self::Planner => RoleKind::Plan.icon(),
            Self::Approval | Self::Decision(_) => RoleKind::Human.icon(),
            Self::Develop(_) => RoleKind::Build.icon(),
            Self::Review(i) => run
                .template()
                .reviews
                .get(i)
                .map_or(Lucide::ListChecks, |_| {
                    RoleKind::review(run.template(), i).icon()
                }),
            Self::Regression => Lucide::ListChecks,
            Self::Commands(_) => Lucide::Terminal,
            Self::Accept => Lucide::ClipboardCheck,
        }
    }
}
