// MEASURE-80: temporary instrumentation for audit finding 80. Removed before the work is done.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

pub(crate) static FRAMES: AtomicU64 = AtomicU64::new(0);
static PANEL: (AtomicU64, AtomicU64) = (AtomicU64::new(0), AtomicU64::new(0));
static RUN_VIEW: (AtomicU64, AtomicU64) = (AtomicU64::new(0), AtomicU64::new(0));

pub(crate) struct Timed(Instant, &'static (AtomicU64, AtomicU64));
pub(crate) fn panel() -> Timed {
    Timed(Instant::now(), &PANEL)
}
pub(crate) fn run_view() -> Timed {
    Timed(Instant::now(), &RUN_VIEW)
}
impl Drop for Timed {
    fn drop(&mut self) {
        self.1.0.fetch_add(1, Relaxed);
        self.1.1.fetch_add(self.0.elapsed().as_nanos() as u64, Relaxed);
    }
}

fn take(counter: &(AtomicU64, AtomicU64)) -> (u64, u64) {
    let (n, nanos) = (counter.0.swap(0, Relaxed), counter.1.swap(0, Relaxed));
    (n, if n == 0 { 0 } else { nanos / n / 1000 })
}

pub(super) fn start(window: &mut Window, cx: &mut Context<Workspace>) {
    let Some(hz) = std::env::var("HIVE_MEASURE_HZ")
        .ok()
        .and_then(|v| v.parse::<f64>().ok())
    else {
        return;
    };
    let runs: u64 = std::env::var("HIVE_MEASURE_RUNS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(250);
    let target = std::env::var("HIVE_MEASURE_TARGET").unwrap_or_else(|_| "sidebar".into());
    let screen = std::env::var("HIVE_MEASURE_SCREEN").unwrap_or_else(|_| "library".into());
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(Duration::from_secs(1));
            let frames = FRAMES.swap(0, Relaxed);
            let (panel, panel_us) = take(&PANEL);
            let (run, run_us) = take(&RUN_VIEW);
            eprintln!(
                "MEASURE frames={frames} panel_renders={panel} panel_avg_us={panel_us} run_renders={run} run_avg_us={run_us}"
            );
        }
    });
    cx.spawn_in(window, async move |this, cx| {
        cx.background_executor().timer(Duration::from_secs(4)).await;
        let opened = this.update_in(cx, |this, window, cx| {
            if this.shown.is_none() {
                let first = this.projects.read(cx).items.first().map(|p| p.path.clone());
                if let Some(path) = first {
                    this.projects.update(cx, |projects, cx| {
                        projects.select_project(&Host::Local, &path, cx)
                    });
                    this.sync_selection(window, cx);
                }
            }
            let Some((Host::Local, owner)) = this.shown.clone() else {
                eprintln!("MEASURE no local project shown");
                return;
            };
            let now = hive_core::pipeline::now_ms();
            AppState::get(cx).store.clone().update(cx, |s, cx| {
                s.edit_pipelines(cx, |lib| {
                    lib.runs.retain(|r| r.id() < 1000 || r.id() >= 1000 + 100_000);
                    for i in 0..runs {
                        let lane = hive_core::pipeline::Lane {
                            name: "Web".into(),
                            project: owner.clone(),
                            base: "HEAD".into(),
                            ..Default::default()
                        };
                        let mut run = hive_core::pipeline::Run::new(
                            1000 + i,
                            owner.clone(),
                            format!("Synthetic task {i}: fix the login timeout and add a regression test"),
                            hive_core::pipeline::Template::default(),
                            vec![lane],
                        )
                        .unwrap();
                        run.record_stage(None, now.saturating_sub((i + 1) * 600_000));
                        if i >= 5 {
                            run.cancel().unwrap();
                            run.record_stage(
                                Some(hive_core::pipeline::Phase::Planning),
                                now.saturating_sub(i * 600_000),
                            );
                        }
                        lib.runs.push(run);
                    }
                })
            });
            if screen != "closed" {
                this.toggle_pipelines(&TogglePipelines, window, cx);
            }
            if screen == "run"
                && let Some(panel) = this.open_pipeline_panel()
            {
                panel.update(cx, |p, cx| p.show_run(1000, None, window, cx));
            }
            eprintln!(
                "MEASURE opened screen={screen} runs={runs} target={target} hz={hz} open={} center={} shown={:?}",
                this.pipelines_open(),
                this.shown_center().is_some(),
                this.shown
            );
        });
        if opened.is_err() {
            return;
        }
        if hz <= 0. {
            return;
        }
        loop {
            cx.background_executor()
                .timer(Duration::from_secs_f64(1. / hz))
                .await;
            let alive = this.update(cx, |this, cx| match target.as_str() {
                "panel" => {
                    if let Some(panel) = this.open_pipeline_panel() {
                        panel.update(cx, |_, cx| cx.notify());
                    }
                }
                _ => this.sidebar.update(cx, |_, cx| cx.notify()),
            });
            if alive.is_err() {
                return;
            }
        }
    })
    .detach();
}
