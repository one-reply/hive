//! Cancel remote file processes when the owning foreground task is dropped.
use gpui_kit::BackgroundExecutor;
use hive_core::command::Cancellation;
use std::sync::Arc;

struct CancelOnDrop(Arc<Cancellation>);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

pub(crate) async fn run<T: Send + 'static>(
    executor: BackgroundExecutor,
    work: impl FnOnce(&Cancellation) -> T + Send + 'static,
) -> T {
    let cancellation = Arc::new(Cancellation::default());
    let guard = CancelOnDrop(cancellation.clone());
    let result = executor.spawn(async move { work(&cancellation) }).await;
    drop(guard);
    result
}
