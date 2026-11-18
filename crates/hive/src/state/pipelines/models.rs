//! Model discovery belongs to the CLI that will run the step, including its local login.
use super::*;

#[derive(Default)]
pub(crate) struct Models {
    pub(crate) options: HashMap<hive_core::agent::AgentKind, Result<Vec<String>, String>>,
    loading: HashSet<hive_core::agent::AgentKind>,
}

impl Pipelines {
    pub(crate) fn load_models(
        &mut self,
        agent: hive_core::agent::AgentKind,
        cx: &mut Context<Self>,
    ) {
        if !self.models.loading.insert(agent) {
            return;
        }
        self.models.options.remove(&agent);
        let task = cx.background_executor().spawn(async move {
            hive_core::agent::launch::pipeline_models(agent).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |p, cx| {
                p.models.loading.remove(&agent);
                p.models.options.insert(agent, result);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
}
