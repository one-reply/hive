//! Saving an image pasted into a task: decoding and writing it happen off the UI thread.
use gpui_kit::{App, Task};
use hive_core::pipeline::images;

/// Saves clipboard image bytes among the task images and resolves to the Markdown reference
/// to insert, or to a message for the user.
pub(crate) fn save_task_image(bytes: Vec<u8>, cx: &App) -> Task<Result<String, String>> {
    cx.background_executor().spawn(async move {
        images::save(&bytes, &images::directory()).map_err(|error| {
            tracing::warn!(%error, "could not save an image pasted into a task");
            error.to_string()
        })
    })
}
