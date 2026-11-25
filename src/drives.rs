//! Encrypted drives: which LUKS container a mount is in, and the
//! bookkeeping that tells a drive that vanished while mounted (suspend
//! switches Thunderbolt and USB devices off; they come back on wake) from
//! one the user unmounted.

use std::collections::{HashMap, HashSet};

/// A UUID compared without case or dashes (device-mapper writes the LUKS
/// UUID without them).
pub fn normal_uuid(uuid: &str) -> String {
    uuid.chars()
        .filter(|c| *c != '-')
        .flat_map(char::to_lowercase)
        .collect()
}

/// The LUKS UUID in a device-mapper UUID cryptsetup gave its device
/// ("CRYPT-LUKS2-b7a69eb5…-name"), normalised.
pub fn luks_of_dm_uuid(dm_uuid: &str) -> Option<String> {
    let rest = dm_uuid.trim().strip_prefix("CRYPT-")?;
    let rest = rest
        .strip_prefix("LUKS2-")
        .or_else(|| rest.strip_prefix("LUKS1-"))?;
    let hex: String = rest.chars().take_while(|c| *c != '-').collect();
    (hex.len() == 32 && hex.chars().all(|c| c.is_ascii_hexdigit())).then(|| normal_uuid(&hex))
}

/// The LUKS container `device` (a /dev path, symlinks allowed) is unlocked
/// from, if it's an unlocked LUKS device.
pub fn luks_of_device(device: &str) -> Option<String> {
    let real = std::fs::canonicalize(device).ok()?;
    let dm = real.file_name()?.to_str()?.to_string();
    let uuid = std::fs::read_to_string(format!("/sys/block/{dm}/dm/uuid")).ok()?;
    luks_of_dm_uuid(&uuid)
}

/// What the watcher knows of the encrypted drives it has seen mounted.
#[derive(Default, Debug)]
pub struct Watch {
    /// Mounted (or just unmounted, not yet settled), by LUKS UUID: the
    /// drive it's on, and whether its mount has gone.
    seen: HashMap<String, (String, bool)>,
    /// Vanished while mounted: to be unlocked and mounted again when back.
    awaiting: HashSet<String>,
}

impl Watch {
    /// The container `luks` is mounted, on `drive`.
    pub fn mounted(&mut self, luks: &str, drive: &str) {
        self.seen
            .insert(normal_uuid(luks), (drive.to_string(), false));
    }

    /// Its mount has gone: unmounted by the user, or the drive went away
    /// (which may be heard of before or after this).
    pub fn unmounted(&mut self, luks: &str) {
        if let Some(entry) = self.seen.get_mut(&normal_uuid(luks)) {
            entry.1 = true;
        }
    }

    /// `drive` has gone: what was mounted on it is awaited.
    pub fn drive_gone(&mut self, drive: &str) {
        let gone: Vec<String> = self
            .seen
            .iter()
            .filter(|(_, (d, _))| d == drive)
            .map(|(k, _)| k.clone())
            .collect();
        for k in gone {
            self.seen.remove(&k);
            self.awaiting.insert(k);
        }
    }

    /// A while after its mount went, `luks`'s drive is still here: the user
    /// unmounted it, and it's not to be mounted again by itself.
    pub fn settle(&mut self, luks: &str) {
        let k = normal_uuid(luks);
        if self.seen.get(&k).is_some_and(|(_, unmounted)| *unmounted) {
            self.seen.remove(&k);
        }
    }

    /// `luks` has come back: whether to unlock and mount it (once).
    pub fn returned(&mut self, luks: &str) -> bool {
        self.awaiting.remove(&normal_uuid(luks))
    }

    /// Whether `luks` is awaited.
    pub fn is_awaited(&self, luks: &str) -> bool {
        self.awaiting.contains(&normal_uuid(luks))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LUKS: &str = "b7a69eb5-80ba-49d7-99c3-41699a35c7fa";

    #[test]
    fn dm_uuids() {
        assert_eq!(
            luks_of_dm_uuid("CRYPT-LUKS2-b7a69eb580ba49d799c341699a35c7fa-ext\n").as_deref(),
            Some("b7a69eb580ba49d799c341699a35c7fa")
        );
        assert_eq!(
            luks_of_dm_uuid("CRYPT-LUKS1-B7A69EB580BA49D799C341699A35C7FA-luks-x").as_deref(),
            Some("b7a69eb580ba49d799c341699a35c7fa")
        );
        assert!(luks_of_dm_uuid("LVM-abc").is_none());
        assert!(luks_of_dm_uuid("CRYPT-PLAIN-foo").is_none());
        assert_eq!(normal_uuid(LUKS), "b7a69eb580ba49d799c341699a35c7fa");
    }

    #[test]
    fn vanished_while_mounted_is_awaited() {
        // Suspend: the drive goes (before or after the mount is cleaned up).
        let mut w = Watch::default();
        w.mounted(LUKS, "/dev/nvme1n1");
        w.drive_gone("/dev/nvme1n1");
        w.unmounted(LUKS);
        assert!(w.returned(LUKS));
        assert!(!w.returned(LUKS), "only once");

        let mut w = Watch::default();
        w.mounted(LUKS, "/dev/nvme1n1");
        w.unmounted(LUKS);
        w.drive_gone("/dev/nvme1n1");
        w.settle(LUKS);
        assert!(w.returned(LUKS));
    }

    #[test]
    fn unmounted_by_the_user_is_left_alone() {
        let mut w = Watch::default();
        w.mounted(LUKS, "/dev/nvme1n1");
        w.unmounted(LUKS);
        w.settle(LUKS);
        // Later unplugged and plugged in again: not mounted by itself.
        w.drive_gone("/dev/nvme1n1");
        assert!(!w.returned(LUKS));
    }

    #[test]
    fn another_drive_going_changes_nothing() {
        let mut w = Watch::default();
        w.mounted(LUKS, "/dev/nvme1n1");
        w.drive_gone("/dev/sda");
        assert!(!w.is_awaited(LUKS));
    }
}
