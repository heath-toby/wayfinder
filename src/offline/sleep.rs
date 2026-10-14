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
/// The download running in a child process, so a pending suspend can stop
/// it: a child reading the mount when the session freezes would make the
/// suspend fail just as an in-process read would.
static CHILD: Mutex<Option<std::process::Child>> = Mutex::new(None);
/// Set when the child was killed for sleep, so its exit isn't taken for a
/// failed download but for an interruption: it's run again after resume.
static KILLED: AtomicBool = AtomicBool::new(false);

/// Longest we hold up a suspend waiting for an in-flight step to finish.
const DRAIN_TIMEOUT: Duration = Duration::from_secs(4);

/// Block until it's safe to touch the FUSE mount, then hold the returned
/// guard for one I/O step (a stat, a directory listing, a download).
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

/// Run `cmd` to completion as one I/O step: it waits while sleep is
/// pending, and if sleep begins while it runs it's killed and run again
/// after resume.
pub(super) fn run_child(
    cmd: &mut std::process::Command,
) -> std::io::Result<std::process::ExitStatus> {
    loop {
        let _io = io_step();
        let child = cmd.spawn()?;
        {
            let mut slot = CHILD.lock().unwrap_or_else(|e| e.into_inner());
            KILLED.store(false, Ordering::SeqCst);
            let child = slot.insert(child);
            // Sleep may have begun since `io_step` looked, before the
            // child was here for `pause` to find: stop it ourselves.
            if PAUSED.load(Ordering::SeqCst) {
                KILLED.store(true, Ordering::SeqCst);
                let _ = child.kill();
            }
        }
        let status = wait_child()?;
        // A child that finished just before it was killed keeps its
        // result: a killed one can't have succeeded.
        if KILLED.swap(false, Ordering::SeqCst) && !status.success() {
            continue;
        }
        return Ok(status);
    }
}

/// Wait for the child in CHILD to exit, without holding the lock while it
/// runs (so `pause` can get at it).
fn wait_child() -> std::io::Result<std::process::ExitStatus> {
    loop {
        {
            let mut slot = CHILD.lock().unwrap_or_else(|e| e.into_inner());
            let Some(child) = slot.as_mut() else {
                return Err(std::io::Error::other("download child went missing"));
            };
            if let Some(status) = child.try_wait()? {
                slot.take();
                return Ok(status);
            }
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Sleep is about to begin: hold off new I/O steps and stop a download in
/// progress.
fn pause() {
    PAUSED.store(true, Ordering::SeqCst);
    if let Some(child) = CHILD.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
        KILLED.store(true, Ordering::SeqCst);
        let _ = child.kill();
    }
}

/// Back from sleep: let I/O steps continue.
fn resume() {
    PAUSED.store(false, Ordering::SeqCst);
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
            pause();
            // Wait for any in-flight step to finish. Once we get the lock,
            // the worker will see PAUSED before its next step.
            let deadline = Instant::now() + DRAIN_TIMEOUT;
            // (Held, that is: a lock poisoned by a panic is free.)
            while matches!(IO_LOCK.try_lock(), Err(std::sync::TryLockError::WouldBlock))
                && Instant::now() < deadline
            {
                tokio::time::sleep(Duration::from_millis(50)).await;
            }
            inhibitor = None;
        } else {
            resume();
            if inhibitor.is_none() {
                inhibitor = take_inhibitor(&conn).await.ok();
            }
        }
    }
    drop(inhibitor);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A download stopped for sleep is run again after resume, and its
    /// caller just sees it succeed.
    #[test]
    fn child_killed_for_sleep_runs_again_after_resume() {
        let started = Instant::now();
        let sleeper = std::thread::spawn(|| {
            std::thread::sleep(Duration::from_millis(200));
            pause();
            std::thread::sleep(Duration::from_millis(300));
            resume();
        });
        let mut cmd = std::process::Command::new("sh");
        cmd.args(["-c", "sleep 0.5"]);
        let status = run_child(&mut cmd).expect("run_child");
        sleeper.join().unwrap();
        assert!(status.success());
        // Killed at 0.2 s, run again from 0.5 s for 0.5 s.
        assert!(started.elapsed() >= Duration::from_millis(950));
        assert!(!KILLED.load(Ordering::SeqCst));
        assert!(CHILD.lock().unwrap().is_none());
    }
}
