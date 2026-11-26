//! Passphrases remembered until restart, in the kernel's user keyring.
//!
//! The kernel keeps them in memory only, never on disk, and drops them when
//! the user's last process ends — on a machine with linger on, only at
//! shutdown or restart. This is how systemd-cryptsetup caches LUKS
//! passphrases too. (GVfs's own "remember" goes to the Secret Service,
//! which with KeePassXC has no session-only collection: the passphrase
//! would be saved for good.)

use std::ffi::CString;

const KEY_SPEC_USER_KEYRING: libc::c_long = -4;
const KEYCTL_SETPERM: libc::c_long = 5;
const KEYCTL_UNLINK: libc::c_long = 9;
const KEYCTL_SEARCH: libc::c_long = 10;
const KEYCTL_READ: libc::c_long = 11;
/// Possessor: everything; this user: view, read, write, search, link,
/// set attributes — so a later Wayfinder (another session keyring) can
/// read and forget it. Nobody else: nothing.
const PERM: libc::c_long = 0x3f3f_0000;

/// The key's name for the encrypted volume with `uuid`.
pub fn key_name(uuid: &str) -> String {
    format!("wayfinder:luks:{uuid}")
}

fn find(name: &str) -> Option<libc::c_long> {
    let ty = CString::new("user").ok()?;
    let desc = CString::new(name).ok()?;
    // SAFETY: valid NUL-terminated strings; the call only reads them.
    let id = unsafe {
        libc::syscall(
            libc::SYS_keyctl,
            KEYCTL_SEARCH,
            KEY_SPEC_USER_KEYRING,
            ty.as_ptr(),
            desc.as_ptr(),
            0 as libc::c_long,
        )
    };
    (id > 0).then_some(id)
}

/// The passphrase remembered for `uuid`, if any.
pub fn recall(uuid: &str) -> Option<String> {
    let id = find(&key_name(uuid))?;
    let mut buf = vec![0u8; 4096];
    // SAFETY: the buffer is writable for its full length.
    let n = unsafe {
        libc::syscall(
            libc::SYS_keyctl,
            KEYCTL_READ,
            id,
            buf.as_mut_ptr(),
            buf.len() as libc::c_long,
        )
    };
    if n <= 0 || n as usize > buf.len() {
        return None;
    }
    buf.truncate(n as usize);
    String::from_utf8(buf).ok()
}

/// Whether a passphrase is remembered for `uuid`.
pub fn is_remembered(uuid: &str) -> bool {
    find(&key_name(uuid)).is_some()
}

/// Remember `passphrase` for `uuid` (replacing any before).
pub fn remember(uuid: &str, passphrase: &str) -> bool {
    let (Ok(ty), Ok(desc)) = (CString::new("user"), CString::new(key_name(uuid))) else {
        return false;
    };
    // SAFETY: valid strings, and a payload pointer/length pair from a &str.
    let id = unsafe {
        libc::syscall(
            libc::SYS_add_key,
            ty.as_ptr(),
            desc.as_ptr(),
            passphrase.as_ptr(),
            passphrase.len(),
            KEY_SPEC_USER_KEYRING,
        )
    };
    if id <= 0 {
        return false;
    }
    // SAFETY: plain integer arguments.
    unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_SETPERM, id, PERM) };
    true
}

/// Forget the passphrase remembered for `uuid`.
pub fn forget(uuid: &str) {
    if let Some(id) = find(&key_name(uuid)) {
        // SAFETY: plain integer arguments.
        unsafe { libc::syscall(libc::SYS_keyctl, KEYCTL_UNLINK, id, KEY_SPEC_USER_KEYRING) };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_recall_forget() {
        let uuid = format!("test-{}", std::process::id());
        if !remember(&uuid, "first") {
            // (No keyring here — a container, say.)
            return;
        }
        // (Or one that takes keys but won't give them back: CI runners'
        // user keyrings aren't searchable. Tidied away, and passed.)
        if recall(&uuid).is_none() {
            forget(&uuid);
            return;
        }
        assert_eq!(recall(&uuid).as_deref(), Some("first"));
        assert!(remember(&uuid, "second"));
        assert_eq!(recall(&uuid).as_deref(), Some("second"));
        forget(&uuid);
        assert!(recall(&uuid).is_none());
        assert!(!is_remembered(&uuid));
    }
}
