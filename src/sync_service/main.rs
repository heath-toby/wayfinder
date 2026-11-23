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
    /// Wayfinder asked for the item holding this path to be synced now;
    /// `true` to hear back even if nothing changed.
    SyncNow(String, bool),
    /// A managed drive was mounted (again).
    Mounted(String),
    /// The managed mount at this mountpoint has lost its login.
    LoginLost(String),
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
        let _ = self.tx.send(Event::SyncNow(path, true));
    }

    /// Like `SyncNow`, but quiet unless something changed (Wayfinder
    /// checks a synced folder when it's opened).
    fn check_now(&self, path: String) {
        let _ = self.tx.send(Event::SyncNow(path, false));
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
    cloud::current_items()
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
                let drives = cloud::load_drives();
                for mounted in mounts.tend(&drives, &tx) {
                    if mounts.reconnecting.remove(&mounted) {
                        let name = drives
                            .iter()
                            .find(|d| d.mountpoint == mounted)
                            .map_or("the drive", |d| d.name());
                        let message = format!("Reconnected to {name}.");
                        log::info!("{message}");
                        report(&conn, &mounted, &message, 2).await;
                    }
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
                    Event::SyncNow(path, tell) => {
                        items = current_items();
                        let real = wayfinder::offline::real_location(&path);
                        if let Some(i) = items.iter().find(|i| real == i.original || real.starts_with(&format!("{}/", i.original))) {
                            let s = state.entry(i.original.clone()).or_default();
                            s.asked |= tell;
                            s.due = Some(Instant::now());
                        }
                    }
                    Event::LoginLost(mountpoint) => {
                        if mounts.restart(&mountpoint).await {
                            let drives = cloud::load_drives();
                            for mounted in mounts.tend(&drives, &tx) {
                                let _ = tx.send(Event::Mounted(mounted));
                            }
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
            // (A due check for an item no longer kept: forget it. Not
            // with no drives read at all — a passing read error.)
            state.retain(|k, _| items.iter().any(|i| i.original == *k));
            if !cloud::load_drives().is_empty() {
                forget_old_items(&items);
            }
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
    let work = cloud::work_dir(item, &state_dir());
    let (ok, out) = rclone(&cloud::bisync_args(item, &link, &work, false)).await;
    log::debug!("bisync {}:\n{out}", item.original);
    let mut outcome = cloud::parse_bisync(&out);
    outcome.ok &= ok;
    if !outcome.ok && !outcome.needs_first_run {
        // (Its output isn't logged otherwise: say why, for next time.)
        let why = outcome.error.clone().unwrap_or_else(|| reason(&out));
        log::warn!("bisync of {} failed: {why}", item.original);
    }
    if !outcome.needs_first_run {
        return outcome;
    }
    // An offline copy made before it was synced both ways was refreshed
    // only when Wayfinder started, so it may be behind the drive: bring
    // it up to date first (Wayfinder's refresh keeps edits made offline).
    // Otherwise the baseline, which deletes nothing, would put back files
    // deleted on the drive since.
    // (Not without a record of what was fetched: then a refresh keeps the
    // newer by date, and an older edit made offline would be lost. Every
    // file that differs keeps both copies instead, below.)
    let entry = wayfinder::offline::load_manifest()
        .get(&item.original)
        .cloned()
        .filter(|_| wayfinder::offline::is_recorded(&item.original));
    if let Some(entry) = entry {
        if !wayfinder::rclone_ops::is_fuse_path(&item.original) {
            return cloud::Outcome {
                error: Some("its first sync waits for the drive to be mounted".into()),
                ..Default::default()
            };
        }
        log::info!(
            "first sync of {}: refreshing its offline copy",
            item.original
        );
        let refreshed =
            tokio::task::spawn_blocking(move || wayfinder::offline::refresh_pin(&entry))
                .await
                .unwrap_or_else(|e| Err(e.to_string()));
        if let Err(e) = refreshed {
            return cloud::Outcome {
                error: Some(format!("couldn't bring its offline copy up to date: {e}")),
                ..Default::default()
            };
        }
    }
    // Whatever still differs in content keeps both copies: the offline
    // one under a conflict name, sent up as a file of its own.
    let (_, check) = rclone(&[
        "check".into(),
        link.to_string_lossy().into_owned(),
        item.remote.clone(),
        "--combined".into(),
        "-".into(),
        "--exclude".into(),
        "*.wayfinder-part".into(),
        "--exclude".into(),
        ".~lock.*#".into(),
    ])
    .await;
    let mut kept_both = 0;
    for rel in cloud::differing_files(&check) {
        let here = item.local.join(&rel);
        let copy = cloud::conflict_copy_name(&here, |p| p.exists());
        match std::fs::rename(&here, &copy) {
            Ok(()) => {
                log::info!(
                    "{} differs from the drive's: kept as {}",
                    here.display(),
                    copy.display()
                );
                kept_both += 1;
            }
            Err(e) => return failed(format!("couldn't keep both copies of {rel}: {e}")),
        }
    }
    log::info!("first sync of {}: building its baseline", item.original);
    let (ok, out) = rclone(&cloud::bisync_args(item, &link, &work, true)).await;
    let mut outcome = cloud::parse_bisync(&out);
    outcome.ok &= ok;
    outcome.conflicts += kept_both;
    outcome
}

fn state_dir() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| PathBuf::from("."))
}

/// rclone's own config file.
fn rclone_config() -> PathBuf {
    std::env::var_os("RCLONE_CONFIG")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("."))
                .join("rclone/rclone.conf")
        })
}

/// A config file holding remote `name` with a login of its own, for
/// `purpose` (the mount, the syncs): made from rclone.conf's section, and
/// made again if the account there changes; the session rclone keeps in it
/// is its own. None to use rclone.conf as it is.
fn session_config(name: &str, purpose: &str) -> Option<PathBuf> {
    let conf = std::fs::read_to_string(rclone_config()).ok()?;
    // (An encrypted config can't be copied from.)
    if conf.contains("RCLONE_ENCRYPT_V0:") {
        return None;
    }
    let section = cloud::config_section(&conf, name)?;
    let fresh = cloud::own_login_section(&section)?;
    let dir = state_dir().join("wayfinder/sessions");
    let path = dir.join(format!("{name}-{purpose}.conf"));
    let current = std::fs::read_to_string(&path).unwrap_or_default();
    let ours = cloud::config_section(&current, name).unwrap_or_default();
    if !cloud::same_account(&ours, &section) {
        use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&dir)
            .ok()?;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)
            .ok()?;
        use std::io::Write;
        f.write_all(fresh.as_bytes()).ok()?;
        log::info!("{name}: a login of its own for the {purpose}");
    }
    Some(path)
}

/// Delete the bisync state and links of items no longer kept offline, so
/// kept again they start afresh.
fn forget_old_items(items: &[SyncItem]) {
    let keep: Vec<String> = items.iter().map(cloud::item_id).collect();
    for dir in ["bisync", "sync"] {
        let Ok(entries) = std::fs::read_dir(state_dir().join("wayfinder").join(dir)) else {
            continue;
        };
        for e in entries.flatten() {
            if !keep.contains(&e.file_name().to_string_lossy().into_owned()) {
                let path = e.path();
                let _ = if path.is_symlink() || path.is_file() {
                    std::fs::remove_file(&path)
                } else {
                    std::fs::remove_dir_all(&path)
                };
            }
        }
    }
}

/// The item's `cloud::short_local` link, made (or put right) to point at
/// its offline copy.
fn short_link(item: &SyncItem) -> std::io::Result<PathBuf> {
    let link = cloud::short_local(item, &state_dir());
    if std::fs::read_link(&link).ok().as_deref() != Some(item.local.as_path()) {
        if let Some(dir) = link.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let _ = std::fs::remove_file(&link);
        std::os::unix::fs::symlink(&item.local, &link)?;
    }
    Ok(link)
}

/// Run rclone: whether it succeeded (its exit status: its messages can't
/// be relied on to say), and its stdout and stderr together.
async fn rclone(args: &[String]) -> (bool, String) {
    let mut cmd = tokio::process::Command::new("rclone");
    // The syncs' own login (see `cloud::own_login_section`), for the drive
    // they work on.
    let drive = cloud::load_drives()
        .into_iter()
        .find(|d| args.iter().any(|a| a.starts_with(&d.remote)));
    if let Some(conf) = drive.and_then(|d| session_config(d.name(), "sync")) {
        cmd.arg("--config").arg(conf);
    }
    match cmd
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output()
        .await
    {
        Ok(o) => {
            let mut s = String::from_utf8_lossy(&o.stdout).into_owned();
            s.push_str(&String::from_utf8_lossy(&o.stderr));
            (o.status.success(), s)
        }
        Err(e) => (false, format!("ERROR : Bisync can't run rclone: {e}")),
    }
}

/// rclone's own reason in its output, for a failed command.
fn reason(output: &str) -> String {
    output
        .lines()
        .rev()
        .find_map(|l| {
            ["CRITICAL: ", "ERROR : ", "Failed to "]
                .iter()
                .find_map(|m| l.split_once(m).map(|(_, why)| why.trim().to_string()))
        })
        .unwrap_or_else(|| "rclone failed".to_string())
}

fn failed(why: impl Into<String>) -> cloud::Outcome {
    cloud::Outcome {
        error: Some(why.into()),
        ..Default::default()
    }
}

/// Where a single file's state at its last sync is kept: "size<TAB>mtime".
fn file_state_path(item: &SyncItem) -> PathBuf {
    state_dir()
        .join("wayfinder")
        .join("files")
        .join(cloud::item_id(item))
}

fn load_file_state(item: &SyncItem) -> Option<cloud::Stamp> {
    let text = std::fs::read_to_string(file_state_path(item)).ok()?;
    let (size, mtime) = text.trim().split_once('\t')?;
    Some(cloud::Stamp {
        size: size.parse().ok()?,
        mtime: mtime.parse().ok()?,
    })
}

fn save_file_state(item: &SyncItem, stamp: cloud::Stamp) {
    let path = file_state_path(item);
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, format!("{}\t{}\n", stamp.size, stamp.mtime));
}

fn local_stamp(path: &std::path::Path) -> Option<cloud::Stamp> {
    let m = std::fs::metadata(path).ok()?;
    let mtime = m
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    Some(cloud::Stamp {
        mtime,
        size: m.len(),
    })
}

/// The remote copy's stamp: `Ok(None)` if it isn't there.
async fn remote_stamp(remote: &str) -> Result<Option<cloud::Stamp>, String> {
    // `lsf --format tsp`: "2026-11-04 14:42:18;123;name", local time.
    let (ok, listing) = rclone(&[
        "lsf".into(),
        "--format".into(),
        "tsp".into(),
        "--files-only".into(),
        remote.to_string(),
    ])
    .await;
    if !ok {
        // (A file that isn't there is "not found": not a failure.)
        if listing.contains("not found") || listing.contains("doesn't exist") {
            return Ok(None);
        }
        return Err(reason(&listing));
    }
    Ok(listing.lines().next().and_then(|l| {
        let mut f = l.splitn(3, ';');
        let t = chrono::NaiveDateTime::parse_from_str(f.next()?, "%Y-%m-%d %H:%M:%S").ok()?;
        let mtime = t.and_local_timezone(chrono::Local).single()?.timestamp();
        Some(cloud::Stamp {
            mtime,
            size: f.next()?.parse().ok()?,
        })
    }))
}

/// Sync a single kept-offline file (see `cloud::file_step`).
async fn sync_file(item: &SyncItem) -> cloud::Outcome {
    if item.local.is_dir() {
        return failed(format!(
            "its offline copy is a folder, not a file ({})",
            item.local.display()
        ));
    }
    let local = local_stamp(&item.local);
    let remote = match remote_stamp(&item.remote).await {
        Ok(r) => r,
        Err(why) => return failed(why),
    };
    let step = cloud::file_step(local, remote, load_file_state(item));
    let one = cloud::Changes {
        modified: 1,
        ..Default::default()
    };
    let local_path = item.local.to_string_lossy().into_owned();
    let mut outcome = cloud::Outcome {
        ok: true,
        ..Default::default()
    };
    let fetch = || {
        vec![
            "copyto".to_string(),
            item.remote.clone(),
            local_path.clone(),
        ]
    };
    match step {
        cloud::FileStep::Nothing => {}
        cloud::FileStep::Send => {
            let (ok, out) =
                rclone(&["copyto".into(), local_path.clone(), item.remote.clone()]).await;
            if !ok {
                return failed(reason(&out));
            }
            outcome.sent = one;
        }
        cloud::FileStep::Fetch => {
            let (ok, out) = rclone(&fetch()).await;
            if !ok {
                return failed(reason(&out));
            }
            outcome.fetched = one;
        }
        cloud::FileStep::KeepBoth => {
            let copy = cloud::conflict_copy_name(&item.local, |p| p.exists());
            if let Err(e) = std::fs::rename(&item.local, &copy) {
                return failed(format!("couldn't keep both copies: {e}"));
            }
            log::info!(
                "{} differs from the drive's: kept as {}",
                local_path,
                copy.display()
            );
            // Both copies on the drive, as bisync leaves a folder's: the
            // offline one beside the original under its conflict name.
            let copy_name = copy
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let remote_copy = match item.remote.rsplit_once('/') {
                Some((dir, _)) => format!("{dir}/{copy_name}"),
                None => match item.remote.split_once(':') {
                    Some((remote, _)) => format!("{remote}:{copy_name}"),
                    None => copy_name.clone(),
                },
            };
            let (ok, out) = rclone(&[
                "copyto".into(),
                copy.to_string_lossy().into_owned(),
                remote_copy,
            ])
            .await;
            if !ok {
                return failed(reason(&out));
            }
            let (ok, out) = rclone(&fetch()).await;
            if !ok {
                return failed(reason(&out));
            }
            outcome.fetched = one;
            outcome.conflicts = 1;
        }
    }
    // As it now is on both sides: the next sync's "last time".
    if let Some(now) = local_stamp(&item.local) {
        save_file_state(item, now);
    }
    outcome
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
    /// When each running mount was started.
    started: HashMap<String, Instant>,
    /// Mounts restarted for a lost login, to say so once they're back.
    pub reconnecting: std::collections::HashSet<String>,
}

impl Mounts {
    /// Start managed mounts that aren't running, notice ones that stopped,
    /// and return the mountpoints newly mounted since the last look.
    fn tend(&mut self, drives: &[Drive], tx: &UnboundedSender<Event>) -> Vec<String> {
        let fuse = fuse_mountpoints();
        for d in drives.iter().filter(|d| d.manage_mount) {
            if let Some(child) = self.running.get_mut(&d.mountpoint) {
                if let Ok(Some(status)) = child.try_wait() {
                    log::warn!("the mount of {} stopped ({status})", d.remote);
                    self.running.remove(&d.mountpoint);
                    let n = self.retry_at.get(&d.mountpoint).map_or(0, |r| r.1) + 1;
                    let wait = Duration::from_secs((10u64 << n.min(5)).min(300));
                    self.retry_at
                        .insert(d.mountpoint.clone(), (Instant::now() + wait, n));
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
            let mut cmd = tokio::process::Command::new("rclone");
            // A login of its own: nothing else can renew it under it.
            if let Some(conf) = session_config(d.name(), "mount") {
                cmd.arg("--config").arg(conf);
            }
            match cmd
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
                // Read for a lost login (and passed on to the journal).
                .stderr(std::process::Stdio::piped())
                .kill_on_drop(false)
                .spawn()
            {
                Ok(mut child) => {
                    log::info!("mounting {} at {}", d.remote, d.mountpoint);
                    if let Some(err) = child.stderr.take() {
                        watch_mount_output(err, d.mountpoint.clone(), tx.clone());
                    }
                    self.started.insert(d.mountpoint.clone(), Instant::now());
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

    /// Stop the mount at `mountpoint` (it's lost its login), to be started
    /// again at once by `tend`. False if it isn't ours, or was only just
    /// started (its last lines may still be coming in).
    async fn restart(&mut self, mountpoint: &str) -> bool {
        if self
            .started
            .get(mountpoint)
            .is_none_or(|t| t.elapsed() < Duration::from_secs(30))
        {
            return false;
        }
        let Some(child) = self.running.remove(mountpoint) else {
            return false;
        };
        log::warn!("the mount at {mountpoint} lost its login: starting it again");
        stop_mount(mountpoint, child).await;
        self.started.remove(mountpoint);
        self.retry_at.remove(mountpoint);
        self.reconnecting.insert(mountpoint.to_string());
        true
    }

    /// Unmount everything the service mounted (rclone unmounts cleanly on
    /// SIGTERM; a kill would leave the mountpoint broken).
    async fn stop_all(&mut self) {
        for (mountpoint, child) in self.running.drain() {
            stop_mount(&mountpoint, child).await;
        }
    }
}

/// Stop a mount (rclone unmounts cleanly on SIGTERM; a kill would leave
/// the mountpoint broken, so then it's unmounted by force).
async fn stop_mount(mountpoint: &str, mut child: tokio::process::Child) {
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
    }
    // (Busy, it may still be mounted: lazily, then.)
    let _ = std::process::Command::new("fusermount3")
        .args(["-uz", mountpoint])
        .stderr(std::process::Stdio::null())
        .status();
}

/// Pass a mount's output on to the journal, and say if it loses its login.
fn watch_mount_output(
    err: tokio::process::ChildStderr,
    mountpoint: String,
    tx: UnboundedSender<Event>,
) {
    tokio::spawn(async move {
        use tokio::io::AsyncBufReadExt;
        let mut lines = tokio::io::BufReader::new(err).lines();
        while let Ok(Some(line)) = lines.next_line().await {
            eprintln!("rclone: {line}");
            if cloud::lost_login(&line) {
                let _ = tx.send(Event::LoginLost(mountpoint.clone()));
            }
        }
    });
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
