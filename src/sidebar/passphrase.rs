//! Asking for an encrypted drive's passphrase.
//!
//! GVfs (as for Nautilus) unlocks a LUKS partition when its volume is
//! mounted, asking the mount operation for the passphrase, then mounts the
//! filesystem inside; unmounting it locks the partition again. GTK's own
//! prompt is replaced by a plain Wayfinder dialog, so it reads well with
//! Orca, Escape cancels, and a wrong passphrase can be asked for again.

use std::cell::{Cell, RefCell};
use std::rc::Rc;

use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

/// What one mount attempt's passphrase prompt saw.
#[derive(Default)]
pub(super) struct Asked {
    /// A passphrase was given (so a failed unlock means a wrong one).
    pub given: Cell<bool>,
    /// It was the remembered one, not typed.
    pub used_remembered: Cell<bool>,
    /// Typed, with "Remember the passphrase until restart" ticked: kept
    /// once it has unlocked the drive (a wrong one never is).
    pub to_remember: RefCell<Option<String>>,
}

/// A mount operation for mounting `label`, over `window`: GTK's for
/// anything else it asks (which processes keep a drive busy, say), ours for
/// a passphrase. `wrong` says the last passphrase didn't work; `remembered`
/// is given without asking.
pub(super) fn mount_operation(
    window: Option<&gtk::Window>,
    label: &str,
    wrong: bool,
    remembered: Option<String>,
    asked: Rc<Asked>,
) -> gtk::MountOperation {
    let op = gtk::MountOperation::new(window);
    let window = window.cloned();
    let label = label.to_string();
    op.connect_ask_password(move |op, _message, _user, _domain, flags| {
        // Only a passphrase alone is ours: a user name, domain or the
        // VeraCrypt options are left to GTK's prompt.
        let ours = gio::AskPasswordFlags::NEED_PASSWORD | gio::AskPasswordFlags::SAVING_SUPPORTED;
        if !ours.contains(flags) {
            return;
        }
        glib::signal::signal_stop_emission_by_name(op, "ask-password");
        // Remembered: given at once, the first time it's asked for.
        if let Some(pass) = remembered.as_deref() {
            if !asked.used_remembered.replace(true) {
                asked.given.set(true);
                op.set_password(Some(pass));
                op.set_password_save(gio::PasswordSave::Never);
                op.reply(gio::MountOperationResult::Handled);
                return;
            }
        }
        ask(window.as_ref(), op, &label, wrong, asked.clone());
    });
    op
}

/// Whether `error` (from mounting) means the passphrase was wrong.
pub(super) fn is_wrong_passphrase(error: &glib::Error, asked: &Asked) -> bool {
    if !asked.given.get() {
        return false;
    }
    let msg = error.message();
    msg.contains("passphrase") || msg.contains("Operation not permitted") || msg.contains("No key")
}

/// Whether `error` is the user cancelling (nothing to report).
pub(super) fn is_cancelled(error: &glib::Error) -> bool {
    error.matches(gio::IOErrorEnum::FailedHandled) || error.matches(gio::IOErrorEnum::Cancelled)
}

fn ask(
    window: Option<&gtk::Window>,
    op: &gtk::MountOperation,
    label: &str,
    wrong: bool,
    asked: Rc<Asked>,
) {
    let dlg = gtk::Window::builder()
        .title(format!("Unlock {label}"))
        .modal(true)
        .default_width(420)
        .resizable(false)
        .build();
    if let Some(w) = window {
        dlg.set_transient_for(Some(w));
    }
    let vbox = gtk::Box::new(gtk::Orientation::Vertical, 10);
    vbox.set_margin_top(14);
    vbox.set_margin_bottom(14);
    vbox.set_margin_start(14);
    vbox.set_margin_end(14);

    let intro_text = if wrong {
        format!("That passphrase didn't unlock {label}. Type it again, or choose Cancel.")
    } else {
        format!("{label} is encrypted. Type its passphrase to unlock and mount it.")
    };
    let intro = gtk::Label::builder()
        .label(&intro_text)
        .wrap(true)
        .xalign(0.0)
        .build();
    vbox.append(&intro);

    let entry_label = gtk::Label::builder()
        .label("_Passphrase")
        .use_underline(true)
        .xalign(0.0)
        .build();
    let entry = gtk::PasswordEntry::builder()
        .show_peek_icon(true)
        .activates_default(true)
        .build();
    entry.update_property(&[
        gtk::accessible::Property::Label(&format!("Passphrase for {label}")),
        gtk::accessible::Property::Description(&intro_text),
    ]);
    entry_label.set_mnemonic_widget(Some(&entry));
    vbox.append(&entry_label);
    vbox.append(&entry);

    // Kept in the kernel's memory (see `keyring`), so gone after a restart.
    let remember = gtk::CheckButton::with_mnemonic("_Remember the passphrase until restart");
    vbox.append(&remember);

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 6);
    buttons.set_halign(gtk::Align::End);
    let cancel = gtk::Button::with_label("Cancel");
    let unlock = gtk::Button::with_label("Unlock");
    unlock.add_css_class("suggested-action");
    buttons.append(&cancel);
    buttons.append(&unlock);
    vbox.append(&buttons);
    dlg.set_child(Some(&vbox));
    dlg.set_default_widget(Some(&unlock));

    // Answered once: Unlock, or anything else that closes the dialog is a
    // cancel.
    let answered = Rc::new(Cell::new(false));
    {
        let op = op.clone();
        let answered = answered.clone();
        dlg.connect_close_request(move |_| {
            if !answered.replace(true) {
                op.reply(gio::MountOperationResult::Aborted);
            }
            glib::Propagation::Proceed
        });
    }
    {
        let dlg = dlg.clone();
        cancel.connect_clicked(move |_| dlg.close());
    }
    {
        let dlg = dlg.clone();
        let op = op.clone();
        let entry = entry.clone();
        let label = label.to_string();
        unlock.connect_clicked(move |_| {
            let pass = entry.text();
            if pass.is_empty() {
                entry.announce(
                    &format!("Type the passphrase for {label} first"),
                    gtk::AccessibleAnnouncementPriority::High,
                );
                entry.grab_focus();
                return;
            }
            answered.set(true);
            asked.given.set(true);
            if remember.is_active() {
                asked.to_remember.replace(Some(pass.to_string()));
            }
            op.set_password(Some(&pass));
            // Asked for every time: never kept in the keyring.
            op.set_password_save(gio::PasswordSave::Never);
            op.reply(gio::MountOperationResult::Handled);
            entry.set_text("");
            dlg.close();
        });
    }
    let keys = gtk::EventControllerKey::new();
    {
        let dlg = dlg.clone();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                dlg.close();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
    }
    dlg.add_controller(keys);
    dlg.present();
    entry.grab_focus();
}
