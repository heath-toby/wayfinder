//! Keeps offline sync off the FUSE mount while the system goes to sleep.
//!
//! Before suspending, systemd freezes the user session, including the
//! rclone process serving the mount. A thread waiting on a FUSE request at
//! that moment can never be frozen, so the kernel aborts the suspend after
//! other processes have already stopped. To avoid that, we hold a logind
//! "delay" sleep inhibitor; on `PrepareForSleep(true)` we pause the sync
//! between I/O steps, then release the inhibitor so sleep can proceed.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, Once};
use std::time::{Duration, Instant};

use futures_util::StreamExt;

static PAUSED: AtomicBool = AtomicBool::new(false);
/// Held by the sync worker for the duration of one FUSE I/O step.
static IO_LOCK: Mutex<()> = Mutex::new(());
static START: Once = Once::new();

/// Longest we hold up a suspend waiting for an in-flight step to finish.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(4);

/// Block until it's safe to touch the FUSE mount, then hold the returned
/// guard for one I/O step (a stat, a directory listing, a chunk read).
pub(super) fn io_step() -> MutexGuard<'static, ()> {
    loop {
        let guard = IO_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        if !PAUSED.load(Ordering::SeqCst) {
            return guard;
        }
        drop(guard);
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Start watching logind for sleep, once per process.
pub(super) fn ensure_watching() {
    START.call_once(|| {
        std::thread::spawn(|| {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    log::warn!("Sleep watcher: no tokio runtime: {e}");
                    return;
                }
            };
            if let Err(e) = rt.block_on(watch()) {
                log::warn!("Sleep watcher stopped: {e}");
            }
        });
    });
}

async fn take_inhibitor(conn: &zbus::Connection) -> zbus::Result<zbus::zvariant::OwnedFd> {
    let reply = conn
        .call_method(
            Some("org.freedesktop.login1"),
            "/org/freedesktop/login1",
            Some("org.freedesktop.login1.Manager"),
            "Inhibit",
            &(
                "sleep",
                "Wayfinder",
                "Pausing offline sync before sleep",
                "delay",
            ),
        )
        .await?;
    reply.body().deserialize()
}

async fn watch() -> zbus::Result<()> {
    let conn = zbus::Connection::system().await?;
    let rule = zbus::MatchRule::builder()
        .msg_type(zbus::message::Type::Signal)
        .sender("org.freedesktop.login1")?
        .interface("org.freedesktop.login1.Manager")?
        .member("PrepareForSleep")?
        .build();
    let mut signals = zbus::MessageStream::for_match_rule(rule, &conn, None).await?;
    let mut inhibitor = Some(take_inhibitor(&conn).await?);

    while let Some(msg) = signals.next().await {
        let Ok(going_down) = msg?.body().deserialize::<bool>() else {
            continue;
        };
        if going_down {
            PAUSED.store(true, Ordering::SeqCst);
            // Wait for any in-flight step to finish. Once we get the lock,
            // the worker will see PAUSED before its next step.
            let deadline = Instant::now() + DRAIN_TIMEOUT;
            while IO_LOCK.try_lock().is_err() && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            inhibitor = None;
        } else {
            PAUSED.store(false, Ordering::SeqCst);
            if inhibitor.is_none() {
                inhibitor = take_inhibitor(&conn).await.ok();
            }
        }
    }
    drop(inhibitor);
    Ok(())
}
