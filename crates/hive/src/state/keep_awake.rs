//! Keeps the Mac out of idle sleep while an agent works, so a long turn is not cut off.

use std::time::Duration;

use gpui_kit::*;
use hive_core::agent::AgentStatus;

use crate::state::{agents::Agents, store::Store};

/// Estimated statuses and hand-offs between agents leave short gaps with nothing working;
/// holding on through them keeps the assertion and its status-bar icon from flickering per turn.
const RELEASE_GRACE: Duration = Duration::from_secs(60);
const REASON: &str = "Hive agents are working";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Decision {
    Hold,
    ReleaseLater,
    ReleaseNow,
}

/// Only work counts: an agent waiting for input or approval is blocked on the person, and
/// one on an SSH host still loses its session when this Mac sleeps.
fn decide(enabled: bool, statuses: impl IntoIterator<Item = AgentStatus>) -> Decision {
    if !enabled {
        Decision::ReleaseNow
    } else if statuses
        .into_iter()
        .any(|status| status == AgentStatus::Working)
    {
        Decision::Hold
    } else {
        Decision::ReleaseLater
    }
}

pub(crate) struct KeepAwake {
    guard: Option<ActivityGuard>,
    /// Dropping it cancels a request still in flight.
    acquiring: Option<Task<()>>,
    release: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl KeepAwake {
    pub(crate) fn new(
        agents: &Entity<Agents>,
        store: &Entity<Store>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = vec![
            cx.observe(agents, {
                let store = store.clone();
                move |this, agents, cx| this.apply(current(&store, &agents, cx), cx)
            }),
            cx.observe(store, {
                let agents = agents.clone();
                move |this, store, cx| this.apply(current(&store, &agents, cx), cx)
            }),
        ];
        let mut this = Self::idle(subscriptions);
        this.apply(current(store, agents, cx), cx);
        this
    }

    fn idle(subscriptions: Vec<Subscription>) -> Self {
        Self {
            guard: None,
            acquiring: None,
            release: None,
            _subscriptions: subscriptions,
        }
    }

    /// Whether idle sleep is being prevented right now.
    pub(crate) fn is_holding(&self) -> bool {
        self.guard.is_some()
    }

    fn apply(&mut self, decision: Decision, cx: &mut Context<Self>) {
        match decision {
            Decision::Hold => {
                self.release = None;
                if self.guard.is_none() && self.acquiring.is_none() {
                    self.acquire(cx);
                }
            }
            Decision::ReleaseLater => {
                let pending = self.guard.is_some() || self.acquiring.is_some();
                if pending && self.release.is_none() {
                    self.release = Some(cx.spawn(async move |this, cx| {
                        cx.background_executor().timer(RELEASE_GRACE).await;
                        let _ = this.update(cx, |this, cx| this.release_now(cx));
                    }));
                }
            }
            Decision::ReleaseNow => self.release_now(cx),
        }
    }

    fn acquire(&mut self, cx: &mut Context<Self>) {
        let request = cx.prevent_idle_sleep(REASON);
        self.acquiring = Some(cx.spawn(async move |this, cx| {
            let result = request.await;
            let _ = this.update(cx, |this, cx| {
                this.acquiring = None;
                match result {
                    // A grace timer started meanwhile still releases it.
                    Ok(guard) => {
                        this.guard = Some(guard);
                        cx.notify();
                    }
                    // The next poll that still sees work asks again.
                    Err(error) => tracing::warn!(%error, "cannot prevent idle sleep"),
                }
            });
        }));
    }

    fn release_now(&mut self, cx: &mut Context<Self>) {
        self.acquiring = None;
        self.release = None;
        if self.guard.take().is_some() {
            cx.notify();
        }
    }
}

fn current(store: &Entity<Store>, agents: &Entity<Agents>, cx: &App) -> Decision {
    let enabled = store.read(cx).state.settings.keep_awake_while_agents_work;
    decide(
        enabled,
        agents.read(cx).all().map(|(_, session)| session.status),
    )
}

#[cfg(test)]
mod tests {
    use super::{Decision, KeepAwake, RELEASE_GRACE, decide};
    use gpui_kit::{AppContext as _, Entity, TestAppContext};
    use hive_core::agent::AgentStatus::*;
    use std::time::Duration;

    #[test]
    fn only_a_working_agent_with_the_setting_on_holds() {
        assert_eq!(decide(true, [Idle, Working]), Decision::Hold);
        assert_eq!(
            decide(true, [Idle, WaitingInput, WaitingApproval, Done, Failed]),
            Decision::ReleaseLater
        );
        assert_eq!(decide(true, []), Decision::ReleaseLater);
        assert_eq!(decide(false, [Working]), Decision::ReleaseNow);
    }

    fn keep_awake(cx: &mut TestAppContext) -> Entity<KeepAwake> {
        cx.new(|_| KeepAwake::idle(Vec::new()))
    }

    fn apply(entity: &Entity<KeepAwake>, decision: Decision, cx: &mut TestAppContext) {
        entity.update(cx, |this, cx| this.apply(decision, cx));
        cx.run_until_parked();
    }

    fn holding(entity: &Entity<KeepAwake>, cx: &mut TestAppContext) -> bool {
        entity.read_with(cx, |this, _| this.is_holding())
    }

    #[gpui::test]
    fn a_working_agent_holds_one_activity(cx: &mut TestAppContext) {
        let entity = keep_awake(cx);
        apply(&entity, Decision::Hold, cx);
        apply(&entity, Decision::Hold, cx);
        assert!(holding(&entity, cx));
        assert_eq!(cx.active_idle_sleep_preventions(), 1);
    }

    #[gpui::test]
    fn release_waits_for_the_grace_period(cx: &mut TestAppContext) {
        let entity = keep_awake(cx);
        apply(&entity, Decision::Hold, cx);
        apply(&entity, Decision::ReleaseLater, cx);
        cx.executor()
            .advance_clock(RELEASE_GRACE - Duration::from_secs(1));
        cx.run_until_parked();
        assert_eq!(cx.active_idle_sleep_preventions(), 1);
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert!(!holding(&entity, cx));
        assert_eq!(cx.active_idle_sleep_preventions(), 0);
    }

    #[gpui::test]
    fn work_resuming_within_the_grace_keeps_the_activity(cx: &mut TestAppContext) {
        let entity = keep_awake(cx);
        apply(&entity, Decision::Hold, cx);
        apply(&entity, Decision::ReleaseLater, cx);
        cx.executor().advance_clock(RELEASE_GRACE / 2);
        apply(&entity, Decision::Hold, cx);
        cx.executor().advance_clock(RELEASE_GRACE * 2);
        cx.run_until_parked();
        assert!(holding(&entity, cx));
        assert_eq!(cx.active_idle_sleep_preventions(), 1);
    }

    #[gpui::test]
    fn turning_the_setting_off_releases_at_once(cx: &mut TestAppContext) {
        let entity = keep_awake(cx);
        apply(&entity, Decision::Hold, cx);
        apply(&entity, Decision::ReleaseNow, cx);
        assert!(!holding(&entity, cx));
        assert_eq!(cx.active_idle_sleep_preventions(), 0);
    }

    #[gpui::test]
    fn failed_acquisition_holds_nothing_and_retries_on_the_next_turn(cx: &mut TestAppContext) {
        let entity = keep_awake(cx);
        cx.set_idle_sleep_prevention_fails(true);
        apply(&entity, Decision::Hold, cx);
        assert!(!holding(&entity, cx));
        cx.set_idle_sleep_prevention_fails(false);
        apply(&entity, Decision::Hold, cx);
        assert!(holding(&entity, cx));
        assert_eq!(cx.active_idle_sleep_preventions(), 1);
    }

    #[gpui::test]
    fn acquisition_finishing_after_work_ended_is_released(cx: &mut TestAppContext) {
        let entity = keep_awake(cx);
        cx.set_idle_sleep_prevention_delay(Duration::from_secs(1));
        entity.update(cx, |this, cx| {
            this.apply(Decision::Hold, cx);
            this.apply(Decision::ReleaseLater, cx);
        });
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert!(holding(&entity, cx));
        cx.executor().advance_clock(RELEASE_GRACE);
        cx.run_until_parked();
        assert!(!holding(&entity, cx));
        assert_eq!(cx.active_idle_sleep_preventions(), 0);
    }

    #[gpui::test]
    fn setting_off_during_acquisition_never_holds(cx: &mut TestAppContext) {
        let entity = keep_awake(cx);
        cx.set_idle_sleep_prevention_delay(Duration::from_secs(1));
        entity.update(cx, |this, cx| {
            this.apply(Decision::Hold, cx);
            this.apply(Decision::ReleaseNow, cx);
        });
        cx.executor().advance_clock(Duration::from_secs(2));
        cx.run_until_parked();
        assert!(!holding(&entity, cx));
        assert_eq!(cx.active_idle_sleep_preventions(), 0);
    }
}
