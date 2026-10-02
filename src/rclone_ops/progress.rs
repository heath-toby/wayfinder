//! Reusable streaming-progress dialog for direct-rclone operations.
//!
//! Drives a non-modal window with a status entry, scrolling log, and Cancel
//! and Close buttons. The caller supplies a worker closure that runs in a
//! background thread, sends `RcloneEvent`s into the provided channel, and
//! finishes by sending `RcloneEvent::Finished`.
//!
//! Used by `pin_with_progress` (single rclone copy) and `delete_with_progress`
//! (sequential rclone deletefile/purge).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::Arc;

use gtk::glib;
use gtk::prelude::*;
use gtk::AccessibleAnnouncementPriority;

use super::{kill, RcloneEvent, RcloneResult};

pub type SummariseFn = fn(&str) -> Option<String>;

/// Strings shown in the progress dialog. `success_message` is a closure so
/// callers can inspect the rclone result (e.g. count conflict-suffix files
/// in the captured stdout) before composing the final announcement.
pub struct ProgressLabels {
    pub title: &'static str,
    pub target_description: String,
    pub initial_status: &'static str,
    pub cancel_label: &'static str,
    pub success_message: Box<dyn FnOnce(&RcloneResult) -> String + 'static>,
    pub fail_prefix: &'static str,
    pub error_prefix: &'static str,
    pub cancel_announce: &'static str,
}

/// Show the dialog and run `worker` in a background thread.
///
/// `worker` receives a sender for `RcloneEvent`s and a cancel flag. It must
/// emit a final `RcloneEvent::Finished` when done. When the user clicks
/// Cancel, the flag is set to true and the most recently started rclone
/// process (per `RcloneEvent::Started`) is killed.
pub fn show<W>(
    parent: &gtk::Window,
    labels: ProgressLabels,
    summarise: SummariseFn,
    on_complete: Option<Box<dyn FnOnce() + 'static>>,
    worker: W,
) where
    W: FnOnce(Sender<RcloneEvent>, Arc<AtomicBool>) + Send + 'static,
{
    let dlg = gtk::Window::builder()
        .title(labels.title)
        .modal(false)
        .transient_for(parent)
        .default_width(600)
        .default_height(400)
        .build();
    dlg.update_property(&[gtk::accessible::Property::Label(labels.title)]);

    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
    vbox.set_margin_top(12);
    vbox.set_margin_bottom(12);
    vbox.set_margin_start(12);
    vbox.set_margin_end(12);

    let label = gtk::Label::builder()
        .label(&labels.target_description)
        .xalign(0.0)
        .wrap(true)
        .build();
    label.update_property(&[gtk::accessible::Property::Label("Operation target")]);

    let progress = gtk::ProgressBar::new();
    progress.set_show_text(false);
    progress.update_property(&[gtk::accessible::Property::Label("Operation progress")]);

    let status = gtk::Entry::builder()
        .editable(false)
        .can_focus(true)
        .text(labels.initial_status)
        .build();
    status.update_property(&[
        gtk::accessible::Property::Label("Current activity"),
        gtk::accessible::Property::Description(
            "Latest message from rclone. Updates live as the operation progresses.",
        ),
    ]);

    let log_view = gtk::TextView::builder()
        .editable(false)
        .monospace(true)
        .wrap_mode(gtk::WrapMode::WordChar)
        .build();
    log_view.update_property(&[gtk::accessible::Property::Label("Operation log")]);
    let log_buffer = log_view.buffer();
    let log_scroll = gtk::ScrolledWindow::builder()
        .child(&log_view)
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();

    let button_box = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    button_box.set_halign(gtk::Align::End);
    button_box.set_margin_top(8);

    let cancel_btn = gtk::Button::with_label(labels.cancel_label);
    cancel_btn.update_property(&[gtk::accessible::Property::Label(labels.cancel_label)]);

    let save_log_btn = gtk::Button::with_label("Save log...");
    save_log_btn.update_property(&[gtk::accessible::Property::Label(
        "Save the rclone log to a file",
    )]);

    let close_btn = gtk::Button::with_label("Close");
    close_btn.set_sensitive(false);
    close_btn.update_property(&[gtk::accessible::Property::Label(
        "Close (enabled when the operation completes)",
    )]);

    button_box.append(&cancel_btn);
    button_box.append(&save_log_btn);
    button_box.append(&close_btn);

    vbox.append(&label);
    vbox.append(&progress);
    vbox.append(&status);
    vbox.append(&log_scroll);
    vbox.append(&button_box);
    dlg.set_child(Some(&vbox));

    {
        let d = dlg.clone();
        close_btn.connect_clicked(move |_| d.close());
    }

    {
        let buf = log_buffer.clone();
        let parent_for_save = parent.clone();
        save_log_btn.connect_clicked(move |btn| {
            let start = buf.start_iter();
            let end = buf.end_iter();
            let text = buf.text(&start, &end, true).to_string();
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let path = format!("/tmp/wayfinder-rclone-{stamp}.log");
            match std::fs::write(&path, &text) {
                Ok(()) => {
                    let msg = format!("Log saved to {path}");
                    parent_for_save
                        .announce(&msg, AccessibleAnnouncementPriority::Medium);
                    btn.set_label("Saved");
                }
                Err(e) => {
                    parent_for_save.announce(
                        &format!("Could not save log: {e}"),
                        AccessibleAnnouncementPriority::High,
                    );
                }
            }
        });
    }

    parent.announce(labels.initial_status, AccessibleAnnouncementPriority::Medium);
    dlg.present();
    status.grab_focus();

    let progress_pulse = progress.clone();
    let pulsing = std::rc::Rc::new(std::cell::Cell::new(true));
    let pulsing_ref = pulsing.clone();
    glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
        if !pulsing_ref.get() {
            return glib::ControlFlow::Break;
        }
        progress_pulse.pulse();
        glib::ControlFlow::Continue
    });

    let cancel_flag = Arc::new(AtomicBool::new(false));
    let (tx, rx) = std::sync::mpsc::channel::<RcloneEvent>();

    let cancel_for_thread = cancel_flag.clone();
    std::thread::spawn(move || worker(tx, cancel_for_thread));

    let pid_holder: std::rc::Rc<std::cell::Cell<Option<u32>>> =
        std::rc::Rc::new(std::cell::Cell::new(None));

    {
        let pid_holder = pid_holder.clone();
        let cancel_flag = cancel_flag.clone();
        let parent = parent.clone();
        let cancel_announce = labels.cancel_announce;
        cancel_btn.connect_clicked(move |btn| {
            cancel_flag.store(true, Ordering::Relaxed);
            if let Some(pid) = pid_holder.get() {
                kill(pid);
            }
            btn.set_sensitive(false);
            parent.announce(cancel_announce, AccessibleAnnouncementPriority::Medium);
        });
    }

    let parent_clone = parent.clone();
    let status_clone = status.clone();
    let close_clone = close_btn.clone();
    let cancel_clone = cancel_btn.clone();
    let pulsing_done = pulsing.clone();
    let log_buffer_clone = log_buffer.clone();
    let log_view_clone = log_view.clone();
    let mut success_message = Some(labels.success_message);
    let fail_prefix = labels.fail_prefix;
    let error_prefix = labels.error_prefix;
    let cancel_flag_poll = cancel_flag.clone();
    let mut on_complete_holder = on_complete;
    glib::timeout_add_local(std::time::Duration::from_millis(150), move || {
        loop {
            match rx.try_recv() {
                Ok(RcloneEvent::Started(pid)) => {
                    pid_holder.set(Some(pid));
                    // If Cancel was clicked before Started arrived, kill now.
                    if cancel_flag_poll.load(Ordering::Relaxed) {
                        kill(pid);
                    }
                }
                Ok(RcloneEvent::Log(line)) => {
                    let mut iter = log_buffer_clone.end_iter();
                    log_buffer_clone.insert(&mut iter, &line);
                    log_buffer_clone.insert(&mut iter, "\n");
                    let mark = log_buffer_clone.create_mark(None, &iter, false);
                    log_view_clone.scroll_to_mark(&mark, 0.0, false, 0.0, 1.0);
                    if let Some(short) = summarise(&line) {
                        status_clone.set_text(&short);
                    }
                }
                Ok(RcloneEvent::Finished(result)) => {
                    pulsing_done.set(false);
                    cancel_clone.set_sensitive(false);
                    close_clone.set_sensitive(true);
                    close_clone.grab_focus();
                    let was_cancelled = cancel_flag_poll.load(Ordering::Relaxed);
                    match result {
                        Ok(r) if r.success && !was_cancelled => {
                            let msg = success_message
                                .take()
                                .map(|f| f(&r))
                                .unwrap_or_else(|| "Done.".to_string());
                            status_clone.set_text(&msg);
                            parent_clone.announce(
                                &msg,
                                AccessibleAnnouncementPriority::Medium,
                            );
                        }
                        Ok(_) if was_cancelled => {
                            let msg = "Cancelled before completion.";
                            status_clone.set_text(msg);
                            parent_clone.announce(
                                msg,
                                AccessibleAnnouncementPriority::Medium,
                            );
                        }
                        Ok(r) => {
                            let tail: String = r
                                .stderr
                                .lines()
                                .rev()
                                .take(3)
                                .collect::<Vec<_>>()
                                .into_iter()
                                .rev()
                                .collect::<Vec<_>>()
                                .join(" | ");
                            let msg = format!("{fail_prefix} {tail}");
                            status_clone.set_text(&msg);
                            parent_clone.announce(&msg, AccessibleAnnouncementPriority::High);
                        }
                        Err(e) => {
                            let msg = format!("{error_prefix} {e}");
                            status_clone.set_text(&msg);
                            parent_clone.announce(&msg, AccessibleAnnouncementPriority::High);
                        }
                    }
                    if let Some(cb) = on_complete_holder.take() {
                        cb();
                    }
                    return glib::ControlFlow::Break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(_) => {
                    pulsing_done.set(false);
                    cancel_clone.set_sensitive(false);
                    close_clone.set_sensitive(true);
                    let msg = "rclone disconnected unexpectedly. Operation may be incomplete.";
                    status_clone.set_text(msg);
                    parent_clone.announce(msg, AccessibleAnnouncementPriority::High);
                    if let Some(cb) = on_complete_holder.take() {
                        cb();
                    }
                    return glib::ControlFlow::Break;
                }
            }
        }
        glib::ControlFlow::Continue
    });
}
