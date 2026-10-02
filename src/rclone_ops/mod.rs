//! Direct rclone operations (copy, delete, move) for paths that live on
//! rclone FUSE mounts.
//!
//! When a file lives on an rclone-mounted directory, going through the FUSE
//! layer for bulk operations is wasteful: every byte traverses kernel ↔
//! userspace ↔ rclone ↔ network instead of letting rclone talk to the remote
//! directly. This module short-circuits that.
//!
//! It detects which remote backs a given local path, then dispatches
//! `rclone copy` / `rclone delete` / etc. directly so we get parallel
//! transfers, real progress, and proper rate-limit handling.

pub mod progress;

use std::path::Path;
use std::process::Command;
use std::sync::mpsc::Sender;

use gtk::prelude::*;

/// A local path resolved to its rclone-remote equivalent.
#[derive(Clone, Debug)]
pub struct RcloneTarget {
    /// Local mountpoint, e.g. `/home/Toby/protonDrive`.
    pub mountpoint: String,
    /// Remote root the mount was created with, e.g. `Proton:` or `Proton:Subdir`.
    pub remote_root: String,
    /// Path relative to the mountpoint, e.g. `Documents/German`.
    pub relative: String,
    /// Full remote spec — what you would pass to `rclone copy`, e.g.
    /// `Proton:Documents/German`.
    pub full_remote_spec: String,
}

/// Resolve a local path to its rclone remote, if it lives inside an rclone
/// FUSE mount. Returns None for paths not on an rclone mount.
pub fn rclone_for_path(local_path: &str) -> Option<RcloneTarget> {
    let mounts = list_rclone_fuse_mounts();
    let mut best: Option<&(String, String)> = None;
    let mut best_len = 0;
    for entry in &mounts {
        // Match either "<mountpoint>" exactly or "<mountpoint>/..."
        let is_prefix = local_path == entry.0
            || local_path.starts_with(&format!("{}/", entry.0));
        if is_prefix && entry.0.len() > best_len {
            best_len = entry.0.len();
            best = Some(entry);
        }
    }
    let (mountpoint, remote_root) = best?;

    let rel = local_path.strip_prefix(mountpoint.as_str())?;
    let rel = rel.trim_start_matches('/').to_string();

    let full = if remote_root.ends_with(':') || remote_root.ends_with('/') {
        format!("{remote_root}{rel}")
    } else if rel.is_empty() {
        remote_root.clone()
    } else {
        format!("{remote_root}/{rel}")
    };

    Some(RcloneTarget {
        mountpoint: mountpoint.clone(),
        remote_root: remote_root.clone(),
        relative: rel,
        full_remote_spec: full,
    })
}

/// Return a list of (mountpoint, remote_root) pairs for currently-running
/// rclone mounts. Reads /proc/mounts to find FUSE mounts, then `ps` to map
/// each FUSE mountpoint back to its rclone remote spec.
fn list_rclone_fuse_mounts() -> Vec<(String, String)> {
    let mut results = Vec::new();

    let Ok(proc_mounts) = std::fs::read_to_string("/proc/mounts") else {
        return results;
    };
    let mut fuse_mounts: Vec<String> = Vec::new();
    for line in proc_mounts.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let mountpoint = parts[1].replace("\\040", " ");
        let fstype = parts[2];
        if fstype.contains("fuse") {
            fuse_mounts.push(mountpoint);
        }
    }
    if fuse_mounts.is_empty() {
        return results;
    }

    let output = match Command::new("ps").arg("-eo").arg("args=").output() {
        Ok(o) => o,
        Err(_) => return results,
    };
    let ps_out = String::from_utf8_lossy(&output.stdout);
    for line in ps_out.lines() {
        let line = line.trim();
        if !line.contains("rclone") {
            continue;
        }
        let tokens: Vec<&str> = line.split_whitespace().collect();

        // Find the rclone binary token. We don't require `mount` to be the
        // immediate next token because users (and systemd units) often pass
        // global flags like `--vfs-cache-mode writes` before the subcommand.
        let mut rclone_idx: Option<usize> = None;
        for (i, tok) in tokens.iter().enumerate() {
            if tok.ends_with("rclone") || *tok == "rclone" {
                rclone_idx = Some(i);
                break;
            }
        }
        let Some(rclone_idx) = rclone_idx else {
            continue;
        };

        // Walk the args after `rclone`, skipping flags (and their values for
        // the unambiguous `--name value` form), until we find a positional
        // subcommand. We only care about `mount`.
        let mut subcommand: Option<usize> = None;
        let mut skip_next = false;
        for (i, tok) in tokens.iter().enumerate().skip(rclone_idx + 1) {
            if skip_next {
                skip_next = false;
                continue;
            }
            if tok.starts_with("--") {
                if !tok.contains('=') {
                    skip_next = true;
                }
                continue;
            }
            if tok.starts_with('-') && tok.len() > 1 {
                // Short flag — assume next token is its value (best-effort;
                // rclone's short flags overwhelmingly take values).
                skip_next = true;
                continue;
            }
            // First positional token is the subcommand.
            subcommand = Some(i);
            break;
        }
        let Some(sub_idx) = subcommand else {
            continue;
        };
        if tokens.get(sub_idx).copied() != Some("mount") {
            continue;
        }

        // After `mount`, gather two positional args: remote spec and mountpoint.
        let mut positional: Vec<&str> = Vec::new();
        let mut skip_next = false;
        for tok in tokens.iter().skip(sub_idx + 1) {
            if skip_next {
                skip_next = false;
                continue;
            }
            if tok.starts_with("--") {
                if !tok.contains('=') {
                    skip_next = true;
                }
                continue;
            }
            if tok.starts_with('-') && tok.len() > 1 {
                skip_next = true;
                continue;
            }
            positional.push(tok);
            if positional.len() >= 2 {
                break;
            }
        }
        if positional.len() < 2 {
            continue;
        }
        let remote = positional[0].to_string();
        let mount = positional[1].to_string();
        if fuse_mounts.iter().any(|m| m == &mount) {
            results.push((mount, remote));
        }
    }
    results
}

/// Whether the rclone binary is on PATH.
pub fn rclone_available() -> bool {
    Command::new("rclone")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// List configured rclone remotes (e.g. ["Proton:", "Drive:"]).
pub fn list_remotes() -> Vec<String> {
    let Some(output) = Command::new("rclone").arg("listremotes").output().ok() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

/// Result of a generic rclone invocation.
#[derive(Debug, Clone)]
pub struct RcloneResult {
    pub success: bool,
    pub stdout: String,
    pub stderr: String,
}

/// A streaming event from a running rclone process.
#[derive(Debug, Clone)]
pub enum RcloneEvent {
    Started(u32),
    Log(String),
    Finished(Result<RcloneResult, String>),
}

/// Spawn `rclone <args>` and stream its output to `tx`. Returns the PID so
/// the caller can kill it. Should be called from a background thread (the
/// caller will typically be inside `std::thread::spawn`).
pub fn run_streaming(args: &[String], tx: Sender<RcloneEvent>) -> Result<u32, String> {
    if !rclone_available() {
        return Err("rclone is not installed".to_string());
    }

    use std::io::{BufRead, BufReader};
    use std::process::Stdio;

    let mut cmd = Command::new("rclone");
    for a in args {
        cmd.arg(a);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to start rclone: {e}"))?;
    let pid = child.id();
    let _ = tx.send(RcloneEvent::Started(pid));

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "no stdout pipe".to_string())?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| "no stderr pipe".to_string())?;

    let stdout_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let stderr_buf = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

    let tx_out = tx.clone();
    let stdout_collect = stdout_buf.clone();
    std::thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(mut buf) = stdout_collect.lock() {
                buf.push_str(&line);
                buf.push('\n');
            }
            let _ = tx_out.send(RcloneEvent::Log(line));
        }
    });

    let tx_err = tx.clone();
    let stderr_collect = stderr_buf.clone();
    std::thread::spawn(move || {
        let reader = BufReader::new(stderr);
        for line in reader.lines().map_while(Result::ok) {
            if let Ok(mut buf) = stderr_collect.lock() {
                buf.push_str(&line);
                buf.push('\n');
            }
            let _ = tx_err.send(RcloneEvent::Log(line));
        }
    });

    std::thread::spawn(move || {
        let status = child.wait();
        let stdout = stdout_buf.lock().map(|b| b.clone()).unwrap_or_default();
        let stderr = stderr_buf.lock().map(|b| b.clone()).unwrap_or_default();
        let result = match status {
            Ok(s) => Ok(RcloneResult {
                success: s.success(),
                stdout,
                stderr,
            }),
            Err(e) => Err(format!("rclone wait failed: {e}")),
        };
        let _ = tx.send(RcloneEvent::Finished(result));
    });

    Ok(pid)
}

/// Kill a running rclone process by PID. Best-effort.
pub fn kill(pid: u32) {
    let _ = Command::new("kill")
        .arg(pid.to_string())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Build the args for `rclone copy <remote_spec> <local_dest>` with sensible
/// defaults: parallel transfers, retries, progress stats.
pub fn copy_args(remote_spec: &str, local_dest: &Path) -> Vec<String> {
    vec![
        "copy".to_string(),
        remote_spec.to_string(),
        local_dest.to_string_lossy().to_string(),
        "--verbose".to_string(),
        "--stats".to_string(),
        "5s".to_string(),
        "--stats-one-line".to_string(),
        "--transfers".to_string(),
        "8".to_string(),
        "--checkers".to_string(),
        "16".to_string(),
        "--retries".to_string(),
        "3".to_string(),
        "--low-level-retries".to_string(),
        "10".to_string(),
    ]
}

/// Build the args for `rclone copyto <src> <dst>` — single-source,
/// single-destination copy that preserves the basename you pass.
pub fn copyto_args(src_spec: &str, dst_spec: &str) -> Vec<String> {
    vec![
        "copyto".to_string(),
        src_spec.to_string(),
        dst_spec.to_string(),
        "--verbose".to_string(),
        "--stats".to_string(),
        "5s".to_string(),
        "--stats-one-line".to_string(),
        "--transfers".to_string(),
        "8".to_string(),
        "--checkers".to_string(),
        "16".to_string(),
        "--retries".to_string(),
        "3".to_string(),
        "--low-level-retries".to_string(),
        "10".to_string(),
    ]
}

/// Build the args for `rclone moveto <src> <dst>` — single-source,
/// single-destination move (server-side when both are the same remote).
pub fn moveto_args(src_spec: &str, dst_spec: &str) -> Vec<String> {
    vec![
        "moveto".to_string(),
        src_spec.to_string(),
        dst_spec.to_string(),
        "--verbose".to_string(),
        "--stats".to_string(),
        "5s".to_string(),
        "--stats-one-line".to_string(),
        "--transfers".to_string(),
        "8".to_string(),
        "--checkers".to_string(),
        "16".to_string(),
        "--retries".to_string(),
        "3".to_string(),
        "--low-level-retries".to_string(),
        "10".to_string(),
    ]
}

/// Build the args for deleting a remote path. Uses `purge` for directories
/// (recursive) and `deletefile` for single files.
pub fn delete_args(remote_spec: &str, is_directory: bool) -> Vec<String> {
    if is_directory {
        vec![
            "purge".to_string(),
            remote_spec.to_string(),
            "--verbose".to_string(),
            "--retries".to_string(),
            "3".to_string(),
            "--low-level-retries".to_string(),
            "10".to_string(),
        ]
    } else {
        vec![
            "deletefile".to_string(),
            remote_spec.to_string(),
            "--verbose".to_string(),
            "--retries".to_string(),
            "3".to_string(),
            "--low-level-retries".to_string(),
            "10".to_string(),
        ]
    }
}

/// Whether `path` is inside a FUSE filesystem mountpoint (rclone, sshfs,
/// gvfs, etc.). Reads `/proc/mounts` and finds the longest matching prefix,
/// then checks its fstype. Uses an exact-or-`mp/` prefix test so that
/// `/home/Toby/protonDrive_backup` does not falsely match a mountpoint at
/// `/home/Toby/protonDrive`.
pub fn is_fuse_path(path: &str) -> bool {
    fstype_for_path(path)
        .map(|t| t.contains("fuse"))
        .unwrap_or(false)
}

/// Whether `path` is specifically inside an rclone FUSE mount, per
/// `/proc/mounts` fstype `fuse.rclone`. This is the kernel's view, which
/// is more reliable than parsing `ps` output.
pub fn is_rclone_fuse_path(path: &str) -> bool {
    fstype_for_path(path)
        .map(|t| t == "fuse.rclone" || t.starts_with("fuse.rclone"))
        .unwrap_or(false)
}

/// All rclone FUSE mountpoints currently visible in `/proc/mounts`.
/// Returned as raw mountpoint paths (e.g. `/home/Toby/protonDrive`).
pub fn current_rclone_fuse_mountpoints() -> Vec<String> {
    let Ok(contents) = std::fs::read_to_string("/proc/mounts") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for line in contents.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let mountpoint = parts[1].replace("\\040", " ");
        let fstype = parts[2];
        if fstype == "fuse.rclone" || fstype.starts_with("fuse.rclone") {
            out.push(mountpoint);
        }
    }
    out
}

/// Return the fstype string from `/proc/mounts` for the longest mountpoint
/// containing `path`, or `None` if no mountpoint matches.
fn fstype_for_path(path: &str) -> Option<String> {
    let contents = std::fs::read_to_string("/proc/mounts").ok()?;
    let mut best_len = 0;
    let mut best_fstype: Option<String> = None;
    for line in contents.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let mountpoint = parts[1].replace("\\040", " ");
        let fstype = parts[2];
        let is_prefix = path == mountpoint
            || path.starts_with(&format!("{mountpoint}/"));
        if is_prefix && mountpoint.len() > best_len {
            best_len = mountpoint.len();
            best_fstype = Some(fstype.to_string());
        }
    }
    best_fstype
}

/// Pick the right spec for an rclone command argument: returns the rclone
/// remote spec if `local_path` is on a recognised rclone mount, else the
/// local path unchanged. rclone treats both forms uniformly.
pub fn spec_for_path(local_path: &str) -> String {
    rclone_for_path(local_path)
        .map(|t| t.full_remote_spec)
        .unwrap_or_else(|| local_path.to_string())
}

/// Run a sequence of rclone invocations one after another. Forwards Started
/// and Log events from each subprocess into the outer channel, and aggregates
/// the per-step results into a single final `RcloneResult`.
///
/// Each step is `(label, args)`. The label is logged before the step runs
/// so progress markers appear in the dialog as `[i/N] label`.
fn run_serial(
    steps: Vec<(String, Vec<String>)>,
    tx: &std::sync::mpsc::Sender<RcloneEvent>,
    cancel: &std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> RcloneResult {
    let mut overall_ok = true;
    let mut stdout = String::new();
    let mut stderr = String::new();
    let total = steps.len();
    let mut aborted = false;
    for (i, (label, args)) in steps.into_iter().enumerate() {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            overall_ok = false;
            break;
        }
        let _ = tx.send(RcloneEvent::Log(format!(
            "[{}/{}] {}",
            i + 1,
            total,
            label
        )));

        let (sub_tx, sub_rx) = std::sync::mpsc::channel::<RcloneEvent>();
        if let Err(e) = run_streaming(&args, sub_tx) {
            // If rclone fails to spawn at all, every subsequent step would
            // fail the same way — abort the batch.
            overall_ok = false;
            stderr.push_str(&format!("Failed to start rclone: {e}\n"));
            break;
        }
        loop {
            match sub_rx.recv() {
                Ok(RcloneEvent::Started(pid)) => {
                    let _ = tx.send(RcloneEvent::Started(pid));
                }
                Ok(RcloneEvent::Log(line)) => {
                    let _ = tx.send(RcloneEvent::Log(line));
                }
                Ok(RcloneEvent::Finished(r)) => {
                    match r {
                        Ok(res) => {
                            if !res.success {
                                overall_ok = false;
                            }
                            stdout.push_str(&res.stdout);
                            stderr.push_str(&res.stderr);
                        }
                        Err(e) => {
                            overall_ok = false;
                            stderr.push_str(&format!("rclone error: {e}\n"));
                        }
                    }
                    break;
                }
                Err(_) => {
                    // The subprocess's stdout/stderr/wait threads dropped
                    // their senders before sending Finished. Treat this as
                    // a fatal abort: don't continue to the next step in
                    // case the user kill -9'd rclone or something equally
                    // bad happened.
                    overall_ok = false;
                    aborted = true;
                    stderr.push_str("rclone subprocess channel closed; aborting remaining steps\n");
                    break;
                }
            }
        }
        if aborted {
            break;
        }
    }
    RcloneResult {
        success: overall_ok,
        stdout,
        stderr,
    }
}

// ---------------------------------------------------------------------------
// High-level entry points: pin and delete via rclone-direct.
// ---------------------------------------------------------------------------

/// Pin a local path for offline use by running `rclone copy` directly
/// against the underlying remote.
///
/// The caller should have already verified `rclone_for_path(local_path)` is
/// `Some` and `rclone_available()` is true.
pub fn pin_with_progress(
    local_path: &str,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    let Some(target) = rclone_for_path(local_path) else {
        parent_window.announce(
            "This path is not on an rclone mount, cannot pin via rclone.",
            gtk::AccessibleAnnouncementPriority::High,
        );
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    };

    let cache_path = crate::offline::cache_path_for(local_path);
    let Some(cache_parent) = cache_path.parent().map(|p| p.to_path_buf()) else {
        parent_window.announce(
            "Pin failed: cannot determine cache location.",
            gtk::AccessibleAnnouncementPriority::High,
        );
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    };
    if let Err(e) = std::fs::create_dir_all(&cache_parent) {
        parent_window.announce(
            &format!("Pin failed: cannot create cache dir: {e}"),
            gtk::AccessibleAnnouncementPriority::High,
        );
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    }

    let args = copy_args(&target.full_remote_spec, &cache_path);
    let labels = progress::ProgressLabels {
        title: "Downloading for offline use",
        target_description: format!(
            "Downloading {} from {} for offline use",
            local_path, target.full_remote_spec
        ),
        initial_status: "Starting rclone copy. Listing remote contents — this can take a moment.",
        cancel_label: "Cancel download",
        success_message: Box::new(|_| "Download complete. Available offline.".to_string()),
        fail_prefix: "Download failed. Last messages:",
        error_prefix: "Download error:",
        cancel_announce: "Cancelling download. Waiting for rclone to exit.",
    };

    progress::show(
        parent_window,
        labels,
        summarise_copy_line,
        on_complete,
        move |tx, _cancel| {
            if let Err(e) = run_streaming(&args, tx.clone()) {
                let _ = tx.send(RcloneEvent::Finished(Err(e)));
            }
        },
    );
}

/// Delete one or more local paths by running `rclone deletefile`/`purge`
/// directly against the underlying remote(s).
///
/// Each path must resolve via `rclone_for_path`. Paths are processed
/// sequentially. The Cancel button kills the current rclone process and
/// stops the loop.
pub fn delete_with_progress(
    paths: Vec<(String, bool)>, // (local_path, is_directory)
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    // Resolve every path up front. If any can't be resolved, refuse.
    let mut resolved: Vec<(String, RcloneTarget, bool)> = Vec::with_capacity(paths.len());
    let mut on_complete = on_complete;
    for (local, is_dir) in &paths {
        match rclone_for_path(local) {
            Some(t) => resolved.push((local.clone(), t, *is_dir)),
            None => {
                parent_window.announce(
                    &format!(
                        "Cannot delete {local} via rclone — path is not on a recognised rclone mount."
                    ),
                    gtk::AccessibleAnnouncementPriority::High,
                );
                if let Some(cb) = on_complete.take() {
                    cb();
                }
                return;
            }
        }
    }

    let total = resolved.len();
    let target_description = if total == 1 {
        format!("Deleting {}", resolved[0].0)
    } else {
        format!("Deleting {total} item(s)")
    };

    let labels = progress::ProgressLabels {
        title: "Deleting from remote",
        target_description,
        initial_status: "Starting rclone delete.",
        cancel_label: "Cancel delete",
        success_message: Box::new(move |_| {
            if total == 1 {
                "Deleted.".to_string()
            } else {
                format!("Deleted {total} item(s).")
            }
        }),
        fail_prefix: "Delete failed. Last messages:",
        error_prefix: "Delete error:",
        cancel_announce: "Cancelling delete. Waiting for rclone to exit.",
    };

    let steps: Vec<(String, Vec<String>)> = resolved
        .iter()
        .map(|(local, target, is_dir)| {
            let label = format!("Deleting {} ({})", local, target.full_remote_spec);
            (label, delete_args(&target.full_remote_spec, *is_dir))
        })
        .collect();

    progress::show(
        parent_window,
        labels,
        summarise_delete_line,
        on_complete,
        move |tx, cancel| {
            let result = run_serial(steps, &tx, &cancel);
            let _ = tx.send(RcloneEvent::Finished(Ok(result)));
        },
    );
}

/// Copy one or more local paths into a destination directory using direct
/// `rclone copyto` calls. Each source is processed sequentially, with
/// rclone's own `--transfers` parallelism within each source.
///
/// The caller should have checked that at least one of `sources` or
/// `dest_dir` lives on a recognised rclone mount.
pub fn copy_paths_with_progress(
    sources: Vec<String>,
    dest_dir: String,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    let total = sources.len();
    if total == 0 {
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    }

    let labels = progress::ProgressLabels {
        title: "Copying via rclone",
        target_description: if total == 1 {
            format!("Copying {} into {}", sources[0], dest_dir)
        } else {
            format!("Copying {total} item(s) into {dest_dir}")
        },
        initial_status: "Starting copy.",
        cancel_label: "Cancel copy",
        success_message: Box::new(move |_| {
            if total == 1 {
                "Copy complete.".to_string()
            } else {
                format!("Copied {total} item(s).")
            }
        }),
        fail_prefix: "Copy failed. Last messages:",
        error_prefix: "Copy error:",
        cancel_announce: "Cancelling copy.",
    };

    let steps: Vec<(String, Vec<String>)> = sources
        .iter()
        .map(|src| build_copy_or_move_step(src, &dest_dir, false))
        .collect();

    progress::show(
        parent_window,
        labels,
        summarise_copy_line,
        on_complete,
        move |tx, cancel| {
            let result = run_serial(steps, &tx, &cancel);
            let _ = tx.send(RcloneEvent::Finished(Ok(result)));
        },
    );
}

/// Move one or more local paths into a destination directory using direct
/// `rclone moveto` calls. Within the same remote this is server-side and
/// nearly instant; cross-remote falls back to copy-then-delete.
pub fn move_paths_with_progress(
    sources: Vec<String>,
    dest_dir: String,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    let total = sources.len();
    if total == 0 {
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    }

    let labels = progress::ProgressLabels {
        title: "Moving via rclone",
        target_description: if total == 1 {
            format!("Moving {} into {}", sources[0], dest_dir)
        } else {
            format!("Moving {total} item(s) into {dest_dir}")
        },
        initial_status: "Starting move.",
        cancel_label: "Cancel move",
        success_message: Box::new(move |_| {
            if total == 1 {
                "Move complete.".to_string()
            } else {
                format!("Moved {total} item(s).")
            }
        }),
        fail_prefix: "Move failed. Last messages:",
        error_prefix: "Move error:",
        cancel_announce: "Cancelling move.",
    };

    let steps: Vec<(String, Vec<String>)> = sources
        .iter()
        .map(|src| build_copy_or_move_step(src, &dest_dir, true))
        .collect();

    progress::show(
        parent_window,
        labels,
        summarise_copy_line,
        on_complete,
        move |tx, cancel| {
            let result = run_serial(steps, &tx, &cancel);
            let _ = tx.send(RcloneEvent::Finished(Ok(result)));
        },
    );
}

/// Rename a path via direct `rclone moveto`. Within the same remote this
/// is server-side and nearly instant. `new_path` is the full new path,
/// including the new basename.
pub fn rename_with_progress(
    old_path: String,
    new_path: String,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    let src_spec = spec_for_path(&old_path);
    let dst_spec = spec_for_path(&new_path);

    let labels = progress::ProgressLabels {
        title: "Renaming via rclone",
        target_description: format!("Renaming {old_path} to {new_path}"),
        initial_status: "Starting rename.",
        cancel_label: "Cancel rename",
        success_message: Box::new(|_| "Rename complete.".to_string()),
        fail_prefix: "Rename failed. Last messages:",
        error_prefix: "Rename error:",
        cancel_announce: "Cancelling rename.",
    };

    let label = format!("Renaming {src_spec} to {dst_spec}");
    let steps = vec![(label, moveto_args(&src_spec, &dst_spec))];

    progress::show(
        parent_window,
        labels,
        summarise_copy_line,
        on_complete,
        move |tx, cancel| {
            let result = run_serial(steps, &tx, &cancel);
            let _ = tx.send(RcloneEvent::Finished(Ok(result)));
        },
    );
}

/// Batch rename: take many `(old_path, new_path)` pairs and run them
/// sequentially through `rclone moveto`. Pairs whose endpoints aren't on
/// rclone mounts are passed through verbatim and rclone treats them as
/// local-to-local operations.
pub fn batch_rename_with_progress(
    pairs: Vec<(String, String)>,
    parent_window: &gtk::Window,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
) {
    let total = pairs.len();
    if total == 0 {
        if let Some(cb) = on_complete {
            cb();
        }
        return;
    }

    let labels = progress::ProgressLabels {
        title: "Renaming via rclone",
        target_description: format!("Renaming {total} item(s)"),
        initial_status: "Starting batch rename.",
        cancel_label: "Cancel rename",
        success_message: Box::new(move |_| format!("Renamed {total} item(s).")),
        fail_prefix: "Rename failed. Last messages:",
        error_prefix: "Rename error:",
        cancel_announce: "Cancelling rename.",
    };

    let steps: Vec<(String, Vec<String>)> = pairs
        .iter()
        .map(|(old, new)| {
            let src_spec = spec_for_path(old);
            let dst_spec = spec_for_path(new);
            let label = format!("Renaming {src_spec} to {dst_spec}");
            (label, moveto_args(&src_spec, &dst_spec))
        })
        .collect();

    progress::show(
        parent_window,
        labels,
        summarise_copy_line,
        on_complete,
        move |tx, cancel| {
            let result = run_serial(steps, &tx, &cancel);
            let _ = tx.send(RcloneEvent::Finished(Ok(result)));
        },
    );
}

/// Build a single (label, args) step for a copy or move from `src` into
/// `dest_dir`. The destination spec preserves the source basename.
fn build_copy_or_move_step(
    src: &str,
    dest_dir: &str,
    is_move: bool,
) -> (String, Vec<String>) {
    let basename = std::path::Path::new(src)
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| src.to_string());
    let dst_path = format!("{}/{}", dest_dir.trim_end_matches('/'), basename);
    let src_spec = spec_for_path(src);
    let dst_spec = spec_for_path(&dst_path);
    let verb = if is_move { "Moving" } else { "Copying" };
    let label = format!("{verb} {src_spec} to {dst_spec}");
    let args = if is_move {
        moveto_args(&src_spec, &dst_spec)
    } else {
        copyto_args(&src_spec, &dst_spec)
    };
    (label, args)
}

fn summarise_copy_line(line: &str) -> Option<String> {
    if line.starts_with("Transferred:") {
        return Some(line.trim().to_string());
    }
    if let Some(idx) = line.find(": Copied") {
        let path = &line[..idx];
        let path = path.rsplit(": ").next().unwrap_or(path);
        return Some(format!("Copied {path}"));
    }
    if line.contains("Listing files") || line.contains("Building file lists") {
        return Some("Listing files on remote...".to_string());
    }
    if line.contains("There was nothing to transfer") {
        return Some("Already up to date.".to_string());
    }
    None
}

fn summarise_delete_line(line: &str) -> Option<String> {
    if let Some(idx) = line.find(": Deleted") {
        let path = &line[..idx];
        let path = path.rsplit(": ").next().unwrap_or(path);
        return Some(format!("Deleted {path}"));
    }
    if line.starts_with('[') && line.contains("Deleting ") {
        // Our own progress markers
        return Some(line.to_string());
    }
    if line.contains("Removing directory") {
        return Some(line.trim().to_string());
    }
    None
}
