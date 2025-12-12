//! Cancellation handling for graceful Ctrl+C cleanup.

use std::sync::atomic::{AtomicBool, Ordering};

static CANCELLED: AtomicBool = AtomicBool::new(false);

/// Check if cancellation has been requested.
pub fn is_cancelled() -> bool {
    CANCELLED.load(Ordering::SeqCst)
}

/// Reset the cancellation flag (for testing or re-use).
pub fn reset() {
    CANCELLED.store(false, Ordering::SeqCst);
}

/// Register the Ctrl+C handler.
///
/// When Ctrl+C is pressed, the cancellation flag is set.
/// Call `is_cancelled()` to check if cancellation was requested.
pub fn register_handler() {
    let _ = ctrlc::set_handler(move || {
        CANCELLED.store(true, Ordering::SeqCst);
    });
}
