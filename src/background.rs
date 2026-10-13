//! Run blocking filesystem work off the GTK main thread.
//!
//! Anything that may touch a FUSE mount can block for as long as the
//! daemon takes to answer, and on the main thread that freezes the window
//! and Orca with it. `run` does the work on its own thread and hands the
//! result back on the main thread.

use std::sync::mpsc;
use std::time::{Duration, Instant};

use gtk::glib;

/// How long work may take before `slow` is called.
const SLOW_AFTER: Duration = Duration::from_millis(400);

/// Run `work` on a new thread, then `done` with its result on the main
/// thread. If the work is still going after a moment, `slow` is called
/// once first (e.g. to say what it's waiting for).
pub fn run<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
    done: impl FnOnce(T) + 'static,
    slow: impl FnOnce() + 'static,
) {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(work());
    });
    let started = Instant::now();
    let mut done = Some(done);
    let mut slow = Some(slow);
    glib::timeout_add_local(Duration::from_millis(30), move || match rx.try_recv() {
        Ok(result) => {
            if let Some(done) = done.take() {
                done(result);
            }
            glib::ControlFlow::Break
        }
        Err(mpsc::TryRecvError::Empty) => {
            if started.elapsed() >= SLOW_AFTER {
                if let Some(slow) = slow.take() {
                    slow();
                }
            }
            glib::ControlFlow::Continue
        }
        Err(mpsc::TryRecvError::Disconnected) => glib::ControlFlow::Break,
    });
}

/// "Waiting for rclone." for a path on a FUSE mount; `None` elsewhere.
pub fn waiting_message(path: &str) -> Option<String> {
    crate::rclone_ops::fuse_daemon_name(path).map(|daemon| format!("Waiting for {daemon}."))
}
