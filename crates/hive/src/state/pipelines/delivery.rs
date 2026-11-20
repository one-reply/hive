//! Taking an accepted run's branches to the hosting service, one explicit request at a time.
use super::*;
use hive_core::git::pull_request::{self, Draft};

/// Current publication/read status. The run keeps the permanent URL separately.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Delivery {
    Checking,
    Ready,
    Existing(pull_request::PullRequest),
    Opening,
    /// A known request whose current checks are unavailable.
    Opened(String),
    Failed(String),
}

impl Pipelines {
    pub(crate) fn delivery(&self, id: u64, lane: usize) -> Option<&Delivery> {
        self.runtime(id)?.deliveries.get(&lane)
    }

    fn remember_request(&mut self, id: u64, lane: usize, url: &str, cx: &mut Context<Self>) {
        if self
            .run(id, cx)
            .is_ok_and(|r| r.request_url(lane) != Some(url))
        {
            self.store.update(cx, |s, cx| {
                s.edit_pipelines(cx, |lib| {
                    if let Some(r) = lib.runs.iter_mut().find(|r| r.id() == id) {
                        r.record_request(lane, url);
                    }
                })
            });
        }
    }

    pub(crate) fn refresh_delivery(&mut self, id: u64, cx: &mut Context<Self>) {
        let Ok(run) = self.run(id, cx).cloned() else {
            return;
        };
        if run.phase() != Phase::Accepted {
            return;
        }
        let run = Arc::new(run);
        for ix in run.delivery_lanes() {
            if matches!(
                self.delivery(id, ix),
                Some(Delivery::Opening | Delivery::Checking)
            ) {
                continue;
            }
            self.runtime_mut(id)
                .deliveries
                .insert(ix, Delivery::Checking);
            let run = run.clone();
            let work = cx.background_executor().spawn(async move {
                if let Some(url) = run.request_url(ix) {
                    return match pull_request::read_link(
                        &Host::Local,
                        &run.lanes()[ix].project,
                        url,
                    ) {
                        Ok(pr) => Delivery::Existing(pr),
                        Err(error) => {
                            tracing::warn!(%error,run=id,"could not refresh request checks");
                            Delivery::Opened(url.to_owned())
                        }
                    };
                }
                let checked = || -> Result<Delivery, String> {
                    let source = workspace::publication(&run, ix).map_err(|e| e.to_string())?;
                    let snapshot = pull_request::read_published(
                        &Host::Local,
                        &run.lanes()[ix].worktree,
                        &source.repository,
                        &source.branch,
                        &source.commit,
                    )
                    .map_err(|e| e.to_string())?;
                    Ok(snapshot
                        .request()
                        .cloned()
                        .map_or(Delivery::Ready, Delivery::Existing))
                };
                checked().unwrap_or_else(Delivery::Failed)
            });
            cx.spawn(async move |this, cx| {
                let result = work.await;
                let _ = this.update(cx, |p, cx| {
                    if let Some(runtime) = p.runtime.get_mut(&id)
                        && runtime.deliveries.get(&ix) == Some(&Delivery::Checking)
                    {
                        let url = match &result {
                            Delivery::Existing(pr) => pr.web_url().map(str::to_owned),
                            _ => None,
                        };
                        runtime.deliveries.insert(ix, result);
                        if let Some(url) = url {
                            p.remember_request(id, ix, &url, cx);
                        }
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    /// Opens a pull or merge request for the branch in `lane`'s worktree. Hive never pushes
    /// for this: an unpublished branch comes back as the hosting CLI's own error.
    pub(crate) fn open_pull_request(
        &mut self,
        id: u64,
        lane: usize,
        draft: Draft,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let run = self.run(id, cx)?;
        if run.phase() != Phase::Accepted {
            return Err("Accept the changes before opening a pull request.".into());
        }
        let worktree = run
            .lanes()
            .get(lane)
            .map(|lane| lane.worktree.clone())
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or("This lane has no worktree.")?;
        let run = run.clone();
        if self.delivery(id, lane) == Some(&Delivery::Opening) {
            return Ok(());
        }
        self.runtime_mut(id)
            .deliveries
            .insert(lane, Delivery::Opening);
        let work = cx.background_executor().spawn(async move {
            let source = workspace::publication(&run, lane).map_err(|e| e.to_string())?;
            let snapshot = pull_request::read_published(
                &Host::Local,
                &worktree,
                &source.repository,
                &source.branch,
                &source.commit,
            )
            .map_err(|e| e.to_string())?;
            if let Some(url) = snapshot.request().and_then(|pr| pr.web_url()) {
                return Ok(url.to_owned());
            }
            if workspace::publication(&run, lane).map_err(|e| e.to_string())? != source {
                return Err("The publishing remote changed. Refresh delivery.".into());
            }
            let provider = snapshot.provider();
            pull_request::create(
                provider,
                &Host::Local,
                &worktree,
                &source.repository,
                &source.branch,
                &draft,
            )
            .map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |pipelines, cx| {
                pipelines.finish_pull_request(id, lane, result, cx)
            });
        })
        .detach();
        cx.notify();
        Ok(())
    }

    pub(super) fn finish_pull_request(
        &mut self,
        id: u64,
        lane: usize,
        result: Result<String, String>,
        cx: &mut Context<Self>,
    ) {
        // A deleted run dropped its runtime; its late answer has nowhere to go.
        let Some(runtime) = self.runtime.get_mut(&id) else {
            return;
        };
        if let Err(error) = &result {
            tracing::warn!(run = id, lane, %error, "pull request was not opened");
        }
        let url = result.as_ref().ok().cloned();
        runtime.deliveries.insert(
            lane,
            match result {
                Ok(url) => Delivery::Opened(url),
                Err(error) => Delivery::Failed(error),
            },
        );
        if let Some(url) = url {
            self.remember_request(id, lane, &url, cx);
        }
        cx.notify();
    }
}
