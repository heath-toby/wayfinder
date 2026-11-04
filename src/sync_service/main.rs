//! `wayfinder-sync`: keeps everything kept offline in step with its cloud
//! drive, both ways, and runs the drives' mounts (see `wayfinder::cloud`).
//!
//! Runs as a systemd user service (Wayfinder's setup guide installs it).
//! It syncs an item:
//! - a few seconds after its offline copy changes (watched with inotify);
//! - every five minutes, since cloud drives like Proton can't say when
//!   they've changed;
//! - when Wayfinder asks (Sync Now, or opening the item), over D-Bus;
//! - when its drive is mounted again.
//!
//! One sync at a time, so a remote's rate limits aren't hit. A failed sync
//! (no network, say) is tried again after a growing pause. What changed is
//! reported as the `Synced` signal on `org.wayfinder.Sync`, which Wayfinder
//! announces; if Wayfinder isn't running, as a desktop notification.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{unbounded_channel, UnboundedSender};
use wayfinder::cloud::{self, Drive, SyncItem};

const BUS_NAME: &str = "org.wayfinder.Sync";
const OBJECT_PATH: &str = "/org/wayfinder/Sync";
/// How often the remotes are checked for changes made elsewhere.
const CHECK_EVERY: Duration = Duration::from_secs(5 * 60);
/// A change is synced once the offline copy has been quiet this long (an
/// app saving a file writes it in several steps).
const QUIET: Duration = Duration::from_secs(4);

enum Event {
    /// Something changed in the offline copies.
    Changed(PathBuf),
    /// Wayfinder asked for the item holding this path to be synced now.
    SyncNow(String),
    /// A managed drive was mounted (again).
    Mounted(String),
}

/// Per-item scheduling.
#[derive(Default)]
struct ItemState {
    /// Changed locally, and when it was last seen changing.
    changed_at: Option<Instant>,
    /// Next routine check.
    due: Option<Instant>,
    /// Asked for: report the result even if nothing changed.
    asked: bool,
    /// Failed runs in a row.
    failures: u32,
}

struct SyncIface {
    tx: UnboundedSender<Event>,
}

#[zbus::interface(name = "org.wayfinder.Sync")]
impl SyncIface {
    /// Sync the item holding `path` (an original or its offline copy) now.
    fn sync_now(&self, path: String) {
        let _ = self.tx.send(Event::SyncNow(path));
    }

    /// The originals of everything synced, one per line.
    fn items(&self) -> Vec<String> {
        current_items().into_iter().map(|i| i.original).collect()
    }

    /// A sync finished: `item` is the original's path; `message` what to
    /// tell the user (empty when nothing changed on a routine check).
    #[zbus(signal)]
    async fn synced(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        item: &str,
        message: &str,
    ) -> zbus::Result<()>;
}

fn current_items() -> Vec<SyncItem> {
    let drives = cloud::load_drives();
    let pins: Vec<(String, bool)> = wayfinder::offline::load_manifest()
        .into_values()
        .map(|p| (p.original_path, p.is_directory))
        .collect();
    let pairs: Vec<(String, String, String)> = wayfinder::rclone_sync::load_pairs()
        .into_values()
        .map(|p| (p.local_path, p.remote, p.sync_local_path))
        .collect();
    cloud::sync_items(&drives, &pins, &pairs, &wayfinder::offline::cache_root())
}

#[tokio::main(flavor = "current_thread")]
async fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let (tx, mut rx) = unbounded_channel::<Event>();

    let conn = match zbus::connection::Builder::session()
        .and_then(|b| b.name(BUS_NAME))
        .and_then(|b| b.serve_at(OBJECT_PATH, SyncIface { tx: tx.clone() }))
    {
        Ok(b) => match b.build().await {
            Ok(c) => c,
            Err(e) => {
                log::error!("can't take {BUS_NAME} (already running?): {e}");
                std::process::exit(1);
            }
        },
        Err(e) => {
            log::error!("D-Bus: {e}");
            std::process::exit(1);
        }
    };

    // Watch the offline copies. (The watcher must stay alive.)
    let cache = wayfinder::offline::cache_root();
    let _ = std::fs::create_dir_all(&cache);
    let watch_tx = tx.clone();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(ev) = res {
            if matches!(ev.kind, notify::EventKind::Access(_)) {
                return;
            }
            for path in ev.paths {
                let _ = watch_tx.send(Event::Changed(path));
            }
        }
    })
    .expect("inotify watcher");
    use notify::Watcher;
    if let Err(e) = watcher.watch(&cache, notify::RecursiveMode::Recursive) {
        log::warn!("can't watch the offline copies: {e}");
    }

    let mut mounts = Mounts::default();
    let mut items = current_items();
    let mut state: HashMap<String, ItemState> = HashMap::new();
    let now = Instant::now();
    for (n, i) in items.iter().enumerate() {
        // (Spread over the first minute, not all at once.)
        state.entry(i.original.clone()).or_default().due =
            Some(now + Duration::from_secs(5 + 10 * n as u64));
    }
    log::info!("syncing {} item(s)", items.len());

    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    let mut mount_check = tokio::time::interval(Duration::from_secs(10));

    loop {
        // Wake for the earliest of: a change gone quiet, a check due.
        let wake = state
            .values()
            .filter_map(|s| match s.changed_at {
                Some(t) => Some(t + QUIET),
                None => s.due,
            })
            .min()
            .unwrap_or_else(|| Instant::now() + CHECK_EVERY);
        tokio::select! {
            _ = sigterm.recv() => break,
            _ = mount_check.tick() => {
                for mounted in mounts.tend(&cloud::load_drives()) {
                    let _ = tx.send(Event::Mounted(mounted));
                }
                continue;
            }
            ev = rx.recv() => {
                let Some(ev) = ev else { break };
                match ev {
                    Event::Changed(path) => {
                        if let Some(i) = cloud::item_for_change(&path, &items) {
                            state.entry(i.original.clone()).or_default().changed_at = Some(Instant::now());
                        }
                    }
                    Event::SyncNow(path) => {
                        items = current_items();
                        let real = wayfinder::offline::real_location(&path);
                        if let Some(i) = items.iter().find(|i| real == i.original || real.starts_with(&format!("{}/", i.original))) {
                            let s = state.entry(i.original.clone()).or_default();
                            s.asked = true;
                            s.due = Some(Instant::now());
                        }
                    }
                    Event::Mounted(mountpoint) => {
                        items = current_items();
                        for i in &items {
                            if i.original == mountpoint || i.original.starts_with(&format!("{mountpoint}/")) {
                                let s = state.entry(i.original.clone()).or_default();
                                s.failures = 0;
                                s.due = Some(Instant::now());
                            }
                        }
                    }
                }
                continue;
            }
            _ = tokio::time::sleep_until(wake.into()) => {}
        }

        // Something is ready: the item kept offline may have changed since
        // the list was read (pinned, unpinned).
        items = current_items();
        let now = Instant::now();
        let ready = items.iter().find(|i| {
            let s = state.entry(i.original.clone()).or_default();
            s.changed_at.is_some_and(|t| now >= t + QUIET)
                || (s.changed_at.is_none() && s.due.is_some_and(|d| now >= d))
        });
        let Some(item) = ready.cloned() else {
            // (A due check for an item no longer kept: forget it.)
            state.retain(|k, _| items.iter().any(|i| i.original == *k));
            for i in &items {
                let s = state.entry(i.original.clone()).or_default();
                if s.due.is_none() {
                    s.due = Some(now + CHECK_EVERY);
                }
            }
            continue;
        };
        let asked = {
            let s = state.entry(item.original.clone()).or_default();
            // Changes during the run mark it again; that run then goes once
            // more (finding nothing, if they were its own).
            s.changed_at = None;
            std::mem::take(&mut s.asked)
        };
        let drive_name = cloud::drive_for(&item.original, &cloud::load_drives())
            .map(|d| d.name().to_string())
            .unwrap_or_else(|| "the drive".to_string());

        let outcome = run_item(&item).await;
        let s = state.entry(item.original.clone()).or_default();
        if outcome.ok {
            s.failures = 0;
            s.due = Some(Instant::now() + CHECK_EVERY);
        } else {
            s.failures += 1;
            let wait = [60, 120, 300, 600][(s.failures as usize - 1).min(3)];
            s.due = Some(Instant::now() + Duration::from_secs(wait));
        }
        let message = match cloud::describe(&item.name(), &drive_name, &outcome) {
            Some(m) => m,
            None if asked => format!("Finished syncing {}: no changes.", item.name()),
            None => String::new(),
        };
        // The mount has the remote's folder listed as it was: have it look
        // again (see `rclone_ops::can_change_directly`).
        if outcome.sent.total() > 0 || outcome.conflicts > 0 {
            let original = item.original.clone();
            let _ = tokio::task::spawn_blocking(move || {
                wayfinder::rclone_ops::forget_in_mounts(&[original])
            })
            .await;
        }
        if !message.is_empty() {
            log::info!("{message}");
        }
        report(&conn, &item.original, &message, s.failures).await;
    }

    log::info!("stopping");
    mounts.stop_all().await;
}

/// Sync one item: bisync for a folder (building its baseline the first
/// time), a newer-wins copy for a single file.
async fn run_item(item: &SyncItem) -> cloud::Outcome {
    if !item.is_dir {
        return sync_file(item).await;
    }
    if !item.local.is_dir() {
        // (Not downloaded yet: Wayfinder's Keep Available Offline does
        // that first.)
        return cloud::Outcome {
            error: Some("its offline copy isn't there yet".into()),
            ..Default::default()
        };
    }
    let link = match short_link(item) {
        Ok(l) => l,
        Err(e) => {
            return cloud::Outcome {
                error: Some(format!("can't make its sync link: {e}")),
                ..Default::default()
            }
        }
    };
    let out = rclone(&cloud::bisync_args(item, &link, false)).await;
    log::debug!("bisync {}:\n{out}", item.original);
    let outcome = cloud::parse_bisync(&out);
    if outcome.needs_first_run {
        log::info!("first sync of {}: building its baseline", item.original);
        return cloud::parse_bisync(&rclone(&cloud::bisync_args(item, &link, true)).await);
    }
    outcome
}

/// The item's `cloud::short_local` link, made (or put right) to point at
/// its offline copy.
fn short_link(item: &SyncItem) -> std::io::Result<PathBuf> {
    let state = dirs::state_dir().unwrap_or_else(|| PathBuf::from("."));
    let link = cloud::short_local(item, &state);
    if std::fs::read_link(&link).ok().as_deref() != Some(item.local.as_path()) {
        if let Some(dir) = link.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&item.local, &link)?;
    }
    Ok(link)
}

/// Run rclone; its stdout and stderr together.
async fn rclone(args: &[String]) -> String {
    match tokio::process::Command::new("rclone")
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
    {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            s
        }
        Err(e) => format!("ERROR : Bisync can't run rclone: {e}"),
    }
}

async fn sync_file(item: &SyncItem) -> cloud::Outcome {
    let local = std::fs::metadata(&item.local).ok().map(|m| {
        let t = m
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        (t, m.len())
    });
    // `lsf --format tsp`: "2026-11-04 14:42:18;123;name", local time.
    let listing = rclone(&[
        "lsf".into(),
        "--format".into(),
        "tsp".into(),
        "--files-only".into(),
        item.remote.clone(),
    ])
    .await;
    let remote = listing.lines().next().and_then(|l| {
        let mut f = l.splitn(3, ';');
        let t = chrono::NaiveDateTime::parse_from_str(f.next()?, "%Y-%m-%d %H:%M:%S").ok()?;
        let t = t.and_local_timezone(chrono::Local).single()?.timestamp();
        Some((t, f.next()?.parse().ok()?))
    });
    if remote.is_none() && listing.contains("ERROR") {
        return cloud::Outcome {
            error: listing
                .lines()
                .find(|l| l.contains("ERROR"))
                .map(|l| l.split_once("ERROR : ").map_or(l, |x| x.1).trim().to_string()),
            ..Default::default()
        };
    }
    let one = cloud::Changes { modified: 1, ..Default::default() };
    let (args, sent) = match cloud::file_sync(local, remote) {
        cloud::FileSync::Nothing => {
            return cloud::Outcome { ok: true, ..Default::default() };
        }
        cloud::FileSync::Fetch => (
            vec!["copyto".into(), item.remote.clone(), item.local.to_string_lossy().into_owned()],
            false,
        ),
        cloud::FileSync::Send => (
            vec!["copyto".into(), item.local.to_string_lossy().into_owned(), item.remote.clone()],
            true,
        ),
    };
    let out = rclone(&args).await;
    let failed = out.lines().find(|l| l.contains("ERROR"));
    cloud::Outcome {
        ok: failed.is_none(),
        sent: if sent && failed.is_none() { one } else { Default::default() },
        fetched: if !sent && failed.is_none() { one } else { Default::default() },
        error: failed.map(|l| l.split_once("ERROR : ").map_or(l, |x| x.1).trim().to_string()),
        ..Default::default()
    }
}

/// Tell Wayfinder (the signal) or, if it isn't running, the desktop (a
/// notification). Repeated failures are told about once, not every retry.
async fn report(conn: &zbus::Connection, item: &str, message: &str, failures: u32) {
    if let Ok(iface) = conn
        .object_server()
        .interface::<_, SyncIface>(OBJECT_PATH)
        .await
    {
        let _ = SyncIface::synced(iface.signal_emitter(), item, message).await;
    }
    if message.is_empty() || failures > 1 {
        return;
    }
    let wayfinder_running = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &("org.wayfinder.FileManager",),
        )
        .await
        .ok()
        .and_then(|r| r.body().deserialize::<bool>().ok())
        .unwrap_or(false);
    if wayfinder_running {
        return;
    }
    let hints: HashMap<&str, zbus::zvariant::Value> = HashMap::new();
    let _ = conn
        .call_method(
            Some("org.freedesktop.Notifications"),
            "/org/freedesktop/Notifications",
            Some("org.freedesktop.Notifications"),
            "Notify",
            &(
                "Wayfinder Sync",
                0u32,
                "folder-remote",
                "Wayfinder Sync",
                message,
                Vec::<&str>::new(),
                hints,
                -1i32,
            ),
        )
        .await;
}

/// The managed drives' mounts, run as children of the service.
#[derive(Default)]
struct Mounts {
    running: HashMap<String, tokio::process::Child>,
    /// When a mount that stopped may be started again.
    retry_at: HashMap<String, (Instant, u32)>,
    /// Mountpoints seen mounted at the last look.
    mounted: Vec<String>,
}

impl Mounts {
    /// Start managed mounts that aren't running, notice ones that stopped,
    /// and return the mountpoints newly mounted since the last look.
    fn tend(&mut self, drives: &[Drive]) -> Vec<String> {
        let fuse = fuse_mountpoints();
        for d in drives.iter().filter(|d| d.manage_mount) {
            if let Some(child) = self.running.get_mut(&d.mountpoint) {
                if let Ok(Some(status)) = child.try_wait() {
                    log::warn!("the mount of {} stopped ({status})", d.remote);
                    self.running.remove(&d.mountpoint);
                    let n = self.retry_at.get(&d.mountpoint).map_or(0, |r| r.1) + 1;
                    let wait = Duration::from_secs((10u64 << n.min(5)).min(300));
                    self.retry_at.insert(d.mountpoint.clone(), (Instant::now() + wait, n));
                }
                continue;
            }
            // Mounted by someone else (by hand, say): leave it be.
            if fuse.contains(&d.mountpoint) {
                continue;
            }
            if self
                .retry_at
                .get(&d.mountpoint)
                .is_some_and(|(at, _)| Instant::now() < *at)
            {
                continue;
            }
            // A mount that died leaves its mountpoint unusable until it's
            // unmounted ("Transport endpoint is not connected").
            let _ = std::process::Command::new("fusermount3")
                .args(["-uz", &d.mountpoint])
                .stderr(std::process::Stdio::null())
                .status();
            let _ = std::fs::create_dir_all(&d.mountpoint);
            match tokio::process::Command::new("rclone")
                .args([
                    "mount",
                    &d.remote,
                    &d.mountpoint,
                    "--vfs-cache-mode",
                    "writes",
                    "--rc",
                    "--rc-addr",
                    &format!("localhost:{}", d.rc_port),
                ])
                // The remote-control login: in the environment, which only
                // this user can read (Wayfinder reads it there too), not on
                // the command line, which anyone can.
                .env("RCLONE_RC_USER", "wayfinder")
                .env("RCLONE_RC_PASS", rc_password())
                .stdin(std::process::Stdio::null())
                .kill_on_drop(false)
                .spawn()
            {
                Ok(child) => {
                    log::info!("mounting {} at {}", d.remote, d.mountpoint);
                    self.running.insert(d.mountpoint.clone(), child);
                }
                Err(e) => log::error!("can't start the mount of {}: {e}", d.remote),
            }
        }
        let newly: Vec<String> = fuse
            .iter()
            .filter(|m| !self.mounted.contains(m))
            .filter(|m| drives.iter().any(|d| d.mountpoint == **m))
            .cloned()
            .collect();
        for m in &newly {
            self.retry_at.remove(m);
        }
        self.mounted = fuse;
        newly
    }

    /// Unmount everything the service mounted (rclone unmounts cleanly on
    /// SIGTERM; a kill would leave the mountpoint broken).
    async fn stop_all(&mut self) {
        for (mountpoint, mut child) in self.running.drain() {
            if let Some(pid) = child.id() {
                let _ = std::process::Command::new("kill")
                    .args(["-TERM", &pid.to_string()])
                    .status();
            }
            if tokio::time::timeout(Duration::from_secs(10), child.wait())
                .await
                .is_err()
            {
                let _ = child.kill().await;
                let _ = std::process::Command::new("fusermount3")
                    .args(["-uz", &mountpoint])
                    .status();
            }
        }
    }
}

/// A random password for this run's remote-control logins.
fn rc_password() -> &'static str {
    static PASS: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    PASS.get_or_init(|| {
        let mut bytes = [0u8; 18];
        if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
            use std::io::Read;
            let _ = f.read_exact(&mut bytes);
        }
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    })
}

fn fuse_mountpoints() -> Vec<String> {
    std::fs::read_to_string("/proc/mounts")
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let mut f = l.split_whitespace();
            let mp = f.nth(1)?;
            let ty = f.next()?;
            ty.contains("fuse")
                .then(|| wayfinder::rclone_ops::decode_mount_path(mp))
        })
        .collect()
}
