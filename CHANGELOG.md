# Changelog

## 2.5.1

### New

- **Reopen All Windows at Startup** — every window of your last session is opened again, each at the folder it last showed, including windows you closed along the way. (Before, this only worked if all the windows were closed at once, and in practice never.) It's a setting, on by default, in the sidebar's menu: press the Menu key or Shift+F10 on any place, or right-click one.

### Fixed

- **Data loss on a mounted drive's own folder** — permanently deleting the folder a drive is mounted on (say `~/protonDrive`) emptied the whole drive; through rclone it ran `rclone purge` on the entire remote. Renaming or moving that folder moved everything off the cloud drive onto local disk, and Replace onto it deleted its contents. Moving a folder with a drive mounted inside it carried that drive's contents along, and moving it to the Bin let Empty Bin delete them later; two-way sync could also reach into a drive mounted inside the synced folder. All of these are now refused, with the advice to unmount the drive first.
- **File chooser could save outside the chosen folder** — a file name suggested by an app (for "Save" or saving several files) such as `../.config/autostart/x` was saved wherever it pointed, giving the app a way to write outside the folder you picked. Only the name is used now, and a typed name can't contain `/`.
- **Offline edits** — files made, changed, moved in or renamed in an offline copy on this computer (opened from the offline copy and saved, say) are kept at every later refresh, instead of being overwritten by the remote version or removed because the remote doesn't have them. Wayfinder tells them apart by keeping a record of what each refresh fetched (`~/.config/wayfinder/offline-base`). The two-way sync settings are written whole, so a crash can't leave them empty. Removing an offline copy while it's being refreshed waits until the refresh is done (it left a stray part behind), and cutting files out of an offline copy is refused.
- **rclone mounts with spaces in the remote's name** — aren't operated on directly (the name can't be read reliably), instead of acting on a shortened, different remote path. A file that has become a folder on the remote (or the reverse) no longer stops the offline refresh. The list of offline items is no longer lost or garbled when several things change it at once.
- **Empty Bin** — folders in the Bin are emptied too; they were left behind while "Bin emptied" was announced.

- **Data loss when pasting a folder onto itself** — pasting a folder into the folder it's already in and choosing Replace deleted it (or left it empty). Replacing something with itself, or with the folder that holds it, is now refused, and so is moving something to where it already is.
- **Deleting a folder with a drive mounted inside it** — a permanent delete would have gone on into the mounted drive and deleted what's on it (a whole remote drive, say). It's now refused until the drive is unmounted. Permanent deletes of local files also no longer freeze the window while they run.
- **The offline copy** — while a folder's offline copy is shown (its drive can't be reached), deleting, renaming, pasting or creating files there — or in folders opened from it — is refused: it changed the copy, not the drive the location bar names. If the offline copy can't be opened, that's now said.
- **Drag and drop onto rclone mounts** — dropping files where one of the same name already exists asks Skip, Replace or Rename, as pasting does, instead of overwriting it; a folder can't be dropped into itself.
- **Window-local cut and paste** — Ctrl+Shift+X then Ctrl+Shift+V no longer crashes Wayfinder partway through the move.
- **Replacing and moving near mounted drives** — choosing Replace over a folder with a drive mounted inside it, or moving such a folder between drives, would have deleted what's on that drive; both now stop with an explanation. The same checks now also see through links (a folder reached through a link is still itself), so neither they nor the Replace-onto-itself check can be got round that way.
- **Replacing a link to a folder** — Replace now replaces the link, instead of overwriting the files in the folder it leads to.
- **Pasting to rclone mounts** — two pasted files with the same name no longer overwrite one another, and folders holding links are copied with their links (rclone silently left them out).
- **Two-way sync through a link** — setting up two-way sync on a folder reached through a link into an rclone mount would have synced the remote with itself (the case Wayfinder guards against for the mount itself); links are now seen through, without touching the mount.
- **Two-way sync and offline copies** — "Remove Offline Copy" on something inside a two-way sync folder deleted sync's local copy, so the next sync deleted those files on the remote too. The offline refresh at startup could also overwrite or remove local changes in a sync folder that hadn't been uploaded yet. Offline copies inside a sync folder are now left to the sync: never refreshed one way, deleted on unpin, or downloaded over. Changing a sync's remote or folder now starts it afresh, instead of every sync failing.
- **Delete in the sidebar** — pressing Delete on a place such as Home moved whatever was selected in the file list to the Bin; it now does nothing there (it still removes a bookmark).
- **Right-clicking a bookmark** — removed it at once; it now opens the bookmark menu (Remove Bookmark, Edit Sidebar). Removing a bookmark is announced, and focus goes to a neighbouring place.
- **Copying a folder into itself** — pasting a folder into one of its own subfolders copied it into itself over and over until the disk filled; it's now refused.
- **Overwriting on rclone mounts** — pasting onto an rclone mount where a file or folder of the same name already exists now asks Skip, Replace or Rename, as everywhere else, instead of silently overwriting it; renaming to a name that's taken, or a batch rename that would give two files the same name, is refused.
- **rclone copies and moves** — empty folders are copied and moved too, and moving a folder no longer leaves an empty copy of its folder tree behind (moved between a disk and a remote, its own emptied folder may stay). When rclone fails, its error message is read out in full.
- **rclone mounts started with flags** — mounts started with options such as `--daemon` or `-v` before the remote name are recognised, so they get the faster direct rclone operations.
- **Copy and move messages** — after "Rename (keep both)", the announcement names the copy's new name; a cancelled copy is recognised as cancelled whatever the system language; and "Waiting for rclone" is said if checking for a clash takes a moment.
- **Responsiveness** — paste, delete and rename on many files no longer run `ps` once per file on the main thread, and emptying the Bin no longer freezes the window.
- **Crash in Go to Location** — pressing Tab after typing a full path inside your home folder (such as `/home/you/Docu`), or after a name with accented letters, could close Wayfinder. Completions now keep the form you typed: `~/…` stays `~/…`, and a full path stays a full path.
- **`~` in Go to Location** — only `~` on its own or `~/…` stands for your home folder, so `~bob` is no longer turned into a path inside it, and a folder next to home such as `/home/youx` is no longer shown as `~x`.
- **Selection after Move to Bin or Delete on a slow mount** — if you have gone to another folder by the time the mount answers, your selection there is left alone instead of jumping. "Folder is now empty" is only said when it's true, and "Moved to Bin" is still announced (after the new folder's "Opened" message rather than over it).
- **Permanent delete after Move to Bin fails** — when a file on a mount can't go to the Bin and you delete it permanently, the selection lands near it instead of at the top of the list, and the selection no longer moves while the "Delete permanently?" question is open.
- **Suspend during an offline refresh** — a download stopped for sleep is fetched again after resume, and the rest of the pinned folder carries on, instead of the refresh stopping there or falling back to copying through the mount. A download that had already finished is kept.
- **Waiting messages** — opening the Bin or Recent Files slowly now says "Waiting for the Bin to open" rather than "trash:"; other locations (such as a folder in the Bin, or `network:`) are named properly too.
- **Permanent delete in a local folder** — focus now lands on the file above the deleted one, rather than on the deleted file itself just before it vanished from the list.
- **Deleted files reappearing** — files moved to the Bin or deleted on a mount while you'd gone to another folder no longer show up again, from the recently-seen list, when you go back. The same goes for files pasted, moved, renamed or created there: you see them as they are now, not under their old names — even if you went back into the folder while the operation was still finishing.
- **Offline copy after unmounting** — going back to a folder on a mount that has since been unmounted shows its "Keep Available Offline" copy again, rather than the folder as last seen and then an empty list.
- **Folders deleted elsewhere** — reopening a recently seen folder on a mount that was deleted on another machine now says it can't be opened (or shows the offline copy), rather than quietly switching to its parent while the location bar still showed the old name.
- **File chooser** — when a recently seen folder turns out to be gone, the file chooser now says so, and keeps its item count up to date when a folder changes.
- **Status bar after a failed refresh** — the item count is brought up to date again, as before 2.5.0, while "Waiting for rclone… Press Alt+Left to go back" stays up for as long as a folder is still opening.
- **Deleted folders reappearing** — going Forward into a folder you've just deleted no longer lists its old contents, and undoing a move to the Bin shows the restored files in their folders straight away.
- **Moved and renamed folders** — going back to a folder you've just moved or renamed on a mount no longer opens its old contents as if it were still there.
- **Move to Bin messages** — when some of the selected files had to be deleted permanently instead, "Moved … to Bin" names a file that really went to the Bin.
- **Cancelled deletes** — cancelling a permanent delete on a mount (or one where nothing could be deleted) no longer moves the selection.
- **Memory** — each file chooser dialog is now freed when it closes; previously the portal kept every one until it quit. A window or file chooser closed while a folder was still opening on a stuck mount stops waiting for it — so a closed window can no longer finish opening that folder later and save it as the folder to start in.
- **Dropping files** — files dragged in from other apps are copied even when their names contain spaces or other special characters, and dropping several files at once copies them all.
- **Tab completion** — Go to Location now says how many entries match (for example "2 matches") instead of "No completions"; `~/.` followed by Tab completes hidden files instead of deleting the dot; and a link to a folder completes with a `/`, ready to go on into it.
- **Pasting over a folder** — after pasting a folder onto one of the same name on a mount, opening it shows what's there now, not what it held before.
- **Bin and Recent Files** — if opening one fails after a wait, the "Waiting for…" message is cleared.
- **`~` with an unusual home setting** — Tab completion of `~/…` works when `$HOME` ends in `/`, or is `/` itself.

- **Permissions** — Apply in Properties no longer removes the setuid, setgid and sticky bits (a shared folder at 2775 stayed 2775, not 775); they can also be set by typing a four-digit mode.
- **Keep Available Offline** — only says "Available offline" when the copy really arrived (a cancelled or failed folder download no longer counts); removing one offline copy no longer deletes another kept inside it; a failed, cancelled or skipped download is reported and not marked as pinned. Pinning a single file through rclone now stores it where Wayfinder looks for it (it was put inside a folder of the same name).
- **Custom actions** — with several files selected, an action that takes one file (such as Extract Here or Checksum) now runs on each of them, not just the first; actions are only offered when they suit every selected file; file names containing `%c`, `%i` or `%k`, and URIs with spaces or `#`, are passed correctly; the progress window can be put away with "Continue in background" or Escape (it used to stay up, blocking the window, for as long as the program ran — a terminal, say).
- **Compress** — file names beginning with `-` are no longer mistaken for options, and an existing archive of the same name is reported instead of being added to or replaced.
- **File chooser (portal)** — Back, Forward and Up no longer crash it. The chosen file is passed to the app correctly when its name has spaces, `#`, `%` or accented letters (it could open the wrong file). Saving over an existing file asks first. A file chosen in one folder is no longer returned after moving to another, the selected file is shown and read out, and when choosing a folder, the folder being shown can be chosen. Enter in the file name field saves; Escape cancels.
- **Dates** — Date Modified (and the date in Properties) is shown in local time; it was shown in UTC, off by the time zone.
- **Links** — pressing Enter on a link to a folder goes into the folder, instead of opening it in another app (it's still listed as a link); a link to a file opens the file, also when it's on a mount.
- **Copying links** — a link is copied as a link, so a folder holding a link to a folder copies instead of failing; onto drives that can't hold links (USB sticks, cloud drives), a link to a file is copied as the file.
- **Right-clicking among marked files** — right-clicking a file that isn't marked acts on that file and clears the marks, instead of acting on the marked files. Right-clicking empty space in the list opens the folder's menu (Paste, New Folder) rather than one for whichever file had focus.
- **Bin names** — files moved to the Bin from another drive are shown and read out by their own names, not a long encoded path.
- **Sorting** — sorting by Size puts folders in order once their sizes are worked out; folders stay above files in a descending sort; and sorting by clicking a column's own header keeps the sort buttons (and what Orca reads) and the saved sort in step.
- **Folder sizes inside network shares and system folders** — sizes are no longer worked out by walking an NFS or SMB share over the network, or into anything mounted inside a folder: `/proc` showed as about 134 million GB. (btrfs subvolumes, which aren't mounted, are still counted.)
- **Folder sizes** — a folder created while you're looking at its parent gets its size worked out (it said "Calculating..." for good), and the last folder's size can no longer go missing.
- **Dragging files** — a file whose name has `#`, `%` or spaces is dragged correctly, and other apps now accept files dragged from Wayfinder.
- **Hidden files** — backup files (`name~`) and names listed in a folder's `.hidden` file are hidden along with dot files, as in other file managers.
- **Opening a folder that can't be read** — a folder on a mount that has stopped answering, or one you don't have permission to read, is reported (or its offline copy shown), instead of quietly opening a folder further up. Back no longer has to be pressed twice when a missing folder opened its parent.
- **Refreshing on return to the window** — a mount that stopped answering no longer stops other folders being checked for changes when you come back to the window; a refresh that fails can't hide a good listing that arrives after it.
- **Folder sizes** — working out sizes is stopped when you move on, instead of earlier walks of big folders piling up.
- **Recent Files** — shows the most recently used files (it could show older ones instead), and a file whose size isn't known is read out as "size unknown", not "0 B". A recent file on a mount that has stopped answering no longer holds up the whole list (it's listed by name if the mount doesn't answer quickly; deleted ones are left out); renamed files stay in Recent Files under their new names, on rclone mounts too.
- **Go to Location** — a relative path (`Projects`, `..`) is taken from the folder you're in; `..` and a trailing `/` are resolved, so paths aren't doubled (`Documents//file`), Up goes up, and your place is kept when you come back. The same in the file chooser's location.
- **Grid view focus** — focus returns to the selected item after opening a folder or switching back to the window.
- **Row labels** — what Orca reads for a file follows changes to it, such as a download growing, instead of keeping its details from when it was first shown.
- **Renaming on case-sensitive drives** — renaming `Report.txt` to `report.txt` where a separate `report.txt` exists is refused, instead of replacing it.
- **Undo move to Bin** — restores the copy just moved there, not an older one from the same place; restoring a file from the Bin whose name has `#` or `%` in it restores that file.
- **Keyboard** — Shift+Tab, and Tab when Back, Forward and Up are unavailable, no longer leave focus stuck in the file list. Shift+Space range selection can no longer freeze the window in an empty or filtered list.
- **The Bin** — items in it can be deleted for good (Delete or Shift+Delete, after asking), and Restore restores every marked item; both used to fail. Opening, renaming, pasting or creating there now says what to do instead of failing or jumping to `/`, and the same goes for pasting into Recent Files. Restoring no longer freezes the window, and Undo names the files it really restored.
- **Renaming and New Folder** — a name containing `/` (or `.` / `..`) is refused; on an rclone mount a rename moved the file into another folder, and New Folder made the folder somewhere else.
- **Copy and cut in the Bin** — say to restore first, instead of putting unusable paths on the clipboard.
- **Pasting from phones and network locations** — a file with no place on disk is reported as not copyable, instead of the dialog closing silently.
- **Compress** — files from different folders (in Recent Files) are no longer compressed from the wrong folder; it says to compress them separately.
- **Scripts in Recent Files** — Nautilus scripts run from Recent Files work (they failed to start).
- **Drives in the sidebar** — activating an unmounted drive mounts and opens it, or says why it couldn't; drives with no place on disk (some phones) are no longer shown as rows that open nothing and eject the wrong drive. Bookmarks saved without a name are read with their folder's real name (not `My%20Docs`).
- **Zoom with several windows** — zooming in one window works (another window's zoom setting overrode it), and resetting returns to the theme's own text size.
- **Filter** — opening a folder while a filter is active no longer announces "Filter cleared" with the old folder's count.
- **rclone mounts with their own config** — aren't operated on directly through the default rclone config (which could have meant a different remote of the same name).
- **Back from the Bin or Recent Files** — goes back to the folder you came from (it skipped it, or went somewhere else); Up does nothing there.
- **Bin menu after editing the sidebar** — right-clicking the Bin keeps its Open Bin / Empty Bin menu after Edit Sidebar.
- **Edit Sidebar** — each place says whether it's shown or hidden, and Enter or Space shows or hides it.
- **Encrypted rclone config** — rclone is never left waiting for a config password, which could freeze the window (opening Set Up Two-Way Sync, for one) or hang an operation.
- **Saving several files from an app** — the file chooser now lets you pick a folder and saves each file there under its own name (numbered if the name is taken), instead of returning a single file for all of them.
- **Sidebar** — Delete does nothing when no place is selected (it could reach the file list); after moving a bookmark with Ctrl+arrow, its row is read with its own name, not its neighbour's; removing the only bookmark leaves focus on a place. In Edit Sidebar, Ctrl+arrow reordering works (the list took the keys), and rows keep saying shown or hidden after a reorder.
- **Keep Available Offline with other offline copies** — pinning a folder that's already kept offline as part of another, or that holds items kept offline separately, is refused with the reason (a fresh download could lose their copies). Removing an offline copy no longer freezes the window while it's deleted.
- **File chooser: typing a location** — Enter on a path that isn't a folder says so, instead of going nowhere and then saving into a folder that doesn't exist.
- **File chooser keyboard** — Tab from the file list goes on to the file name (or the Open button), Shift+Tab back to the location; Enter in the location goes there; Recent Files and the Bin in its sidebar say they can't be opened there. "Save As" on an existing file starts in its folder, with its name.
- **Opening files from the command line** — `wayfinder some/file` uses the app chosen for that file, and never waits on a mount (reached through a link, say) while starting.
- **Bookmarks** — folders with spaces or accented letters are bookmarked correctly (they were written so that GTK apps, and Wayfinder, opened the wrong folder), and such bookmarks made by other apps open properly.
- **"Show in folder"** — from other apps, it now selects the file, and works for files with spaces or accented letters in their path.
- **Open With per file** — the app chosen for a file whose path contains `=` is remembered.
- **Open With** — pressing Enter on "Open With" moves focus into the submenu.
- **Right-click** — right-clicking a file opens the menu for that file; it used to act on whichever file was selected before.
- **After renaming** — the renamed file stays selected, so a key pressed next acts on it and not on its neighbour.
- **Marked files hidden by a search** — copy, cut, delete, Move to Bin, restore and custom actions say the marked files are hidden (and how to show or unmark them), instead of silently doing nothing.
- **Two-way sync with a drive mounted inside** — "Sync now" is refused while a drive is mounted inside the synced folder, as setting up the sync already was.
- **Windows opened by other apps or the saved session** — Back no longer leads to the last folder of an earlier session, which the window never showed (possibly on a slow mount); that goes for restored Bin and Recent windows too.
- **Settings** — saved whole or not at all, so Wayfinder crashing while saving can't leave the settings or per-file apps half written (a settings file kept as a link is still written in place, as before); a last folder whose name ends in a space is reopened, not its parent.
- **Go to Location in the Bin or Recent Files** — Tab no longer completes a name from whatever folder Wayfinder was started in.
- **Rename** — the name before the extension is selected correctly when it has accented letters (`Résumé.pdf` selected `Résumé.p`); all of a hidden file's name (`.bashrc`), and all of a folder's (`v1.2`), is selected.
- **Open Terminal Here** in Recent Files or the Bin says it can't, instead of announcing a terminal opened in `recent:///`; if the terminal fails to start, that's said, and the next terminal is tried if one can't be run. Closed terminals, and scripts and programs run from Wayfinder, no longer linger as defunct processes. A script run in a terminal waits for Enter in any `sh` (with dash, the terminal closed at once). Scripts and programs opened from Wayfinder run in their own folder (a script's `./configure` or `out.log` was looked for, or written, wherever Wayfinder was started). Restoring from the Bin no longer clears files marked meanwhile in another folder.
- **Copying, moving, renaming and pinning on rclone mounts failed with current rclone** — Wayfinder passed `copyto` and `moveto` options they don't take (rclone 1.75 stops with "unknown flag"), so every rclone-direct copy, move, rename and "Keep Available Offline" download failed. Folders now go through `copy`/`move` (keeping their empty subfolders), files through `copyto`/`moveto`.
- **A failed check no longer counts as "nothing there"** — renaming or pasting through rclone, and saving several files from another app, treated a name that couldn't be checked (a slow or failing mount) or a link leading nowhere as free, and could write over what was there.
- **Messages in Rename, Batch Rename and New Folder** ("A name can't contain /", duplicate names, "Enter text to find") are spoken from the dialog, which stays open with focus (Orca could miss them).
- **Context menu** — pressing Menu again inside it no longer opens a second menu that closes at once. Custom actions are refused in the offline copy (their output would have gone into Wayfinder's cache). Coming back to the window keeps focus in the search or location field if that's where it was.
- **Show in folder for a hidden file** (from another app) says the file is hidden and how to show hidden files, instead of selecting another file. **File chooser** — Save in a folder that failed to open says so; a corrected location, entered right after another, is the one gone to.
- **rclone-direct operations on the right files** — a drive mounted inside an rclone mount (sshfs, a USB stick) is no longer taken for part of the remote, where deleting there purged the remote's own folder of that name; only this user's rclone mounts are used (a system service's names remotes from another config); mounts set up to mount on first use (automount) are seen as the rclone mounts they are; an empty folder copied, moved, renamed or kept offline through rclone is made (it said "complete" and wasn't there; a move then removed the original).
- **Keep Available Offline** works for names with a tab or newline in them (they couldn't be removed again). **7-Zip archives** take a `?` or `*` in a name as part of it, not a wildcard.
- **Zip archives keep links as links** (they copied in what a link led to — a big folder, or one on a mount — and dropped broken links), as the tar formats do. **Copying a folder holding a socket or pipe** leaves those out instead of stopping partway (moving one still stops: deleting the source would lose them).
- **Nautilus scripts** get each selected file followed by a newline, as Nautilus gives them (a `while read` loop missed the last one). **Progress windows** say what they're working on (Orca read "Operation target").
- **Compress** — tar formats work with GNU tar when bsdtar isn't installed (they were offered, then failed); an empty archive name, or one with `/`, is refused instead of making a hidden `.zip` or writing elsewhere.
- **rclone progress windows** — results, errors and "log saved" are spoken from the progress window, which has focus (Orca could miss them from the window behind).
- **File chooser: saving under a folder's name** — says it's a folder, instead of offering to "replace" it and handing the app a folder to write to.
- **A drive unmounted cleanly** — opening one of its folders shows that folder's offline copy, not the drive's top folder's, and remembers the folder asked for.
- **Marked files that have gone** (moved away from another window, renamed, deleted) no longer count as marked: actions use the focused file again instead of saying the marked files are hidden.
- **Changing only the case of a name** (`report.txt` to `Report.txt`) works on USB sticks and Windows shares, which ignore case (the name was "already taken" by the file itself).
- **Some files not moved to the Bin, or not deleted** — the message also says how many were.
- **Bookmarking in the offline copy** bookmarks the folder it's a copy of, not Wayfinder's cache.
- **Context menu keys** — Right, Space and typed letters in a file's menu act in the menu; the file list behind it could take them (Right didn't open Open With, Space marked a file), and window shortcuts such as Backspace or Delete no longer act behind an open menu.
- **Rename keeps the extension** — the name without its extension stays selected when the Rename window opens (taking focus selected all of it, so typing a name dropped the extension).
- **Closed windows are freed** — every window's sidebar was kept alive by the drive monitor, and with it the window, its folder and its file monitor; the file chooser leaked likewise.
- **Zoom** — steps are of the theme's own font size (with an 11pt font, the first zoom in did nothing). The shortcuts list says Backspace goes to the parent folder, as it does. "Show folders" for the Bin or Recent Files from another app opens them, not `/`.
- **Keep Available Offline, more** — a pin removed while the startup refresh is running is no longer downloaded again; a mount that appears after Wayfinder started gets its offline copies shown when it's down.
- **Two-way sync of a mount's folder** — it can only be synced with the remote it's mounted from: with any other, its offline copy (the old remote's files) would have been merged into it, however the pairing was set up. It's set up while the drive is mounted (with the drive down, or on the offline copy itself, it's refused: which remote, and which copy, couldn't be told); an existing pairing can still be edited or removed, and "Sync now" refuses to sync the folder of a drive that isn't mounted, or the mount itself.
- **Custom actions could run commands hidden in a file's name** — with an action whose `Exec=` put the file in quotes (`"%f"`), a file named, say, `$(…).txt` ran what was in the brackets, and ordinary names with spaces broke the action. Actions now run as GLib runs them: `Exec=` is split into its arguments first and the files go in as arguments, with no shell reading them. Field codes are filled in unquoted arguments (`%f`, `%f~`, `--input=%f`, and `"%f"` on its own), never inside quoted text, where inline code goes (`sh -c '…'`, `python3 -c '…'`): such an action gets the files as its arguments (`"$1"`), as the bundled actions do. `%%` is a `%`.
- **Custom actions and scripts** — actions run in the folder shown (an `unzip %f` action extracted wherever Wayfinder was started from, not next to the archive); Nautilus scripts get the selected files as arguments (`"$@"`), as in Nautilus; actions for any type (`*/*`; `all/allfiles` for files), for a kind of type (`text/plain` covers scripts and Markdown, but `application/octet-stream` isn't taken to mean every file), or excluding one (`!inode/directory`) show where they should; an action still runs if the folder shown has since been deleted; an action's own `./script` in a folder with a space in its name runs.
- **rclone progress** — "Save log" saves to `~/.cache/wayfinder/`, readable only by you (it was a predictable file in `/tmp`); Cancel can no longer stop an unrelated program that happened to get the number of an rclone step that had finished; long logs no longer slow the window down.
- **Copying and moving folders** — folders copied, or moved to another drive, keep their permissions (a private folder became readable by everyone), and moves keep files' dates. Replace works when a folder replaces a file of the same name or the other way round (it failed). "Keep both" on a hidden file keeps it hidden (`.bashrc (2)`, not ` (2).bashrc`), and a broken link at the destination counts as a name taken, so Replace or Keep both is offered.
- **Lists that change while you read them** — a file appearing in the folder shown no longer re-sorts the whole list (Orca lost its place each time); a refresh that started before files changed no longer brings back ones just deleted or drops ones just made; folder sizes arriving in a folder with thousands of subfolders no longer freeze the window.
- **Opening a file through a link into a mount** (`wayfinder ~/Drive/file`) no longer reads part of it to guess its type.
- **Bookmarks file** — if it can't be read (another app wrote it in another encoding, say), adding, removing or moving a bookmark says so instead of writing a file with only that one change and losing the rest, other apps' bookmarks included. Bookmark changes show in every open window, and no stray separator is left after the last bookmark goes.
- **Properties** — the result of Apply, Open With and Set as Default is spoken from the Properties window (Orca could miss it from the window behind). For a link, the dates shown are the link's own, and no permissions are offered (changing them would have changed the file it leads to). A remembered app that's no longer installed no longer shows the first app in the list as the one in use.
- **File chooser** — choosing Save or Open with a location typed but not yet gone to (Enter) says so, instead of using the folder shown before; checks on a slow mount say "Waiting for rclone."; saving several files with an empty name among them keeps the files and names in step.
- **Ejecting drives** — Eject is in the sidebar's menu (Menu key or Shift+F10 on a drive), since the eject button can't be reached from the keyboard; ejecting says whether it worked, and focus stays in the sidebar when the drive's row goes. Drives that need a password to mount now ask for it.
- **Sidebar order** — with File System hidden, new bookmarks no longer appear below the Bin; drives are listed above Recent instead of between a separator and Recent.
- **Bookmarking the Bin or Recent Files** — Ctrl+D there says only folders can be bookmarked, instead of adding a bookmark that opens nothing.
- **Escape** — closes the Properties, Compress and Edit Sidebar windows, as in the other dialogs.
- **File chooser** — choosing a place in its sidebar, or going to a typed location, moves focus to the files (Tab can't leave the sidebar's list).
- **Requests from other apps** — "show these folders" with several folders opens a window for each (each used to replace the last in one window), and items in several folders likewise; "show properties" now opens the Properties window once the folder is shown.
- **Undo trash and cancelled deletes** — undo says the name a restored file actually went back under; a cancelled delete says how far it got. In rclone progress windows, focus moves to a working button when Cancel is pressed or rclone stops unexpectedly.
- **Shift+Space ranges** — a range runs from the file it was started on, even if files came or went in the list meanwhile (it used the old row, marking the wrong files).
- **Undo after moving to the Bin** — restores only what that move put in the Bin, never an older item that once came from the same place.
- **Offline copies** — the Properties window no longer changes the permissions of an offline copy (the change never reached the drive, and was undone at the next refresh).
- **Bookmarks saved by other apps without a name** — no longer get one written into the bookmarks file when Wayfinder changes the bookmarks.
- **Two-way sync window** — why a sync can't start is spoken from the window itself (Orca could miss it).
- **File chooser** — the Open/Save button has an Alt key, and its spoken name has no stray underscore.
- **The Bin with items from a cloud drive** — opening it no longer reads (downloads) those files to tell their type; it's taken from the name, as in the drive's own folders.
- **Pasting files copied in another app** — when that app leaves out the copy/cut line, the first file is pasted too (it was dropped).
- **Back to a folder that's gone** — when its parent opens instead, Forward no longer stays on, going to the same folder.
- **Recent Files** — with a cloud drive that has dropped, the list no longer waits needlessly before showing.
- **Changes made by other programs** — a file written while the folder was being refreshed no longer goes unseen until the next refresh.
- **Closing a copy, move or delete progress window** (Alt+F4) — cancels, as Cancel does (it went on, unseen); a delete's window stays until the item being deleted is done. (rclone's progress windows can still be closed to let the work run on, saying when it's done.)
- **Copying onto a name that can't be checked** (a mount erroring) — asks, as for a name taken, instead of merging into the folder there; "Keep both" says why it can't (no free number, a name too long, the mount's error) instead of merging into the existing folder.
- **Keeping a folder offline again** — if the old offline copy can't be removed, it says so, instead of downloading beside it and reporting success.
- **Properties** — a link's Created and Last Opened dates are shown; an item in the Bin shows its Original Location; Apply with an invalid octal number says so instead of applying the last valid one.
- **Go to Location** — Tab on `~` completes to `~/`; Escape closes it from its buttons too.
- **Hiding the sidebar** (F3) with focus in it — focus goes to the files, instead of nowhere.
- **Context menus** — Down with no item focused goes to the first item, not the second.
- **Sidebar** — the Menu key (or Shift+F10) on the Bin opens its own menu with Empty Bin, as right-click does; removing a bookmark names it as it's shown.
- **Renaming in Recent Files** — the renamed file is the one selected afterwards (another of the same name, from another folder, could be — and a Delete pressed next would go to it); renaming several there no longer leaves their folders' cached listings showing the old names.
- **Requests from other apps to show a file** — one that couldn't be shown then (the drive was unreachable) is no longer announced on a later, unrelated visit.
- **The Bin in several windows** — emptying it, or restoring from it, in one window shows in the others when you go to them.
- **Delete pressed again** while a file is still going to the Bin (on a slow drive) says so, instead of nothing; Shift+Delete leaves such a file alone instead of deleting it at the same time.
- **Open Terminal Here in the offline copy** is refused, as other changes there are.
- **Compress** — Enter in the archive name creates it, as in the Rename and New Folder dialogs.
- **An rclone mount inside another, or two at one place** — when Wayfinder can't tell which remote the files shown are on (one started with `--config`, say), it no longer acts on them through the other mount's remote, where a delete would have removed a different folder. Copying, pasting or keeping offline a folder with another drive mounted inside it goes through the files as shown, not the remote (which would have fetched what's hidden under that drive).
- **Keep Available Offline pressed again** while it's downloading — says so, instead of starting a second download into the same copy.
- **Suspend** — if the system bus goes away while offline copies are paused for sleep, they're no longer paused for the rest of the session; a download that can't be waited on is stopped, not left running.
- **Two-way sync** — a sync that finishes after its pairing was changed to other folders no longer marks the new pairing as synced (its first sync then failed every time).

### Development

- **Unit tests** — `cargo test` now covers path completion, location naming, selection after removing files, the folder cache, dropped files, custom action commands, offline pins, and restarting an offline download after suspend.

## 2.5.0

### FUSE / rclone mount support

- **The window no longer freezes on a slow mount** — folders are listed in the background. If one takes more than a moment, the status bar shows, and Orca says, "Waiting for rclone to open Documents. Press Alt+Left to go back." Going back, or anywhere else, works straight away; the late listing is ignored. Local folders open exactly as before.
- **Much faster listings on FUSE mounts** — Wayfinder no longer reads the start of files to work out their type there (on rclone, that meant downloading part of every file); the type comes from the file name.
- **Instant back, up and revisits** — recently seen FUSE folders are shown at once and quietly refreshed. Resting on a folder for a moment fetches its contents in advance, so opening it is instant.
- **Faster startup** — the last folder is listed once instead of twice, and checking that it still exists no longer holds up the window.
- **Refresh in place** — after copying, moving, renaming or deleting, the list updates without resetting, so the selection stays where it was.
- **Opening files on a mount** — the app is chosen from the file type without waiting on the mount.
- **Everything else stays responsive too** — Rename, batch rename, New Folder, Move to Bin, Properties (and applying permissions), Keep Available Offline, the check for an existing file when pasting, and Tab completion in Go to Location all wait on the mount in the background, saying "Waiting for rclone." if it takes a moment. Pressing Delete again while a move to Bin is still waiting does nothing, rather than trashing the same files twice.
- **Changes are picked up when you come back** — a folder on a mount is checked for changes whenever the Wayfinder window becomes active again, instead of relying on a file monitor (which never sees changes made elsewhere, and set up its watch on the main thread).
- **Folders that disappear** — if a recently seen folder can no longer be opened (deleted, or the mount has gone), Wayfinder says so and falls back to the offline copy where there is one, instead of showing what it held last time.
- **Safe to suspend** — the "Keep Available Offline" refresh pauses before the computer sleeps, and folder-size calculation never walks into a FUSE or network mount. Previously either could leave Wayfinder stuck waiting on rclone, which made suspend fail.
- **Offline copies are refreshed, not re-downloaded** — at startup only changed files are fetched, into a temporary file that replaces the old copy once complete, so an interrupted refresh no longer loses the offline copy. Pins are left alone while their mount is down. Changed files are fetched with `rclone copyto` straight from the remote (or with `cp` through the mount when rclone can't), so Wayfinder never holds a file on the mount open — on FUSE, every program Wayfinder started while one was open had to wait for rclone first, which could stall the window for seconds.

### Fixed

- **Focus starts in the file list** — at startup, and after Go to Location, focus is on the selected file instead of the sidebar's first place.
- **Sorting after changes** — a file whose date or size changes moves to its place when the list is sorted by that column.
- **Opening a file from the command line** — `wayfinder some/file` now opens that file, rather than whichever item happened to be selected in its folder.
- **Recent Files with spaces in their names** — no longer silently left out.
- **File chooser (portal)** — Alt+Left, Alt+Right and Alt+Up now go back, forward and up.

## 2.4.0

### New features

- **Copy and paste files with other apps** — Ctrl+C, Ctrl+X and the Cut and Copy menu items now put files on the system clipboard in the same formats Nautilus and Dolphin use. Pasting into Signal or WhatsApp attaches the files, and other file managers can paste them, including cut-to-move.
- **Paste files copied in other apps** — Ctrl+V pastes files copied or cut in Nautilus, Dolphin or another copy of Wayfinder. When the system clipboard holds no files, Wayfinder's own clipboard is used as before. The window-local clipboard (Ctrl+Shift+C/X/V) stays private.
- **rclone integration** — "Keep Available Offline" pinning for files on rclone mounts, two-way sync setup with `rclone bisync`, and direct `rclone` copy, move and delete with a streaming progress dialog instead of going through the FUSE mount.
- **Run executables on open** — opening an executable file runs it instead of opening it in a text editor. Scripts run in a terminal so their output is visible.
- **Folder-based custom actions** — custom actions can live in their own subfolder, with `./`-relative `Exec=` and `TryExec=` paths resolved against that folder.

### FUSE / rclone mount support

- **Background deletes on FUSE mounts** — deleting on sshfs and other FUSE mounts runs in a background thread with a progress dialog instead of freezing the window.

### Fixed

- **Copy Path and Copy Name** — now actually reach the clipboard. On Wayland, setting the clipboard after the context menu closed was silently ignored by the compositor.
- **Stale paste** — after another app takes over the clipboard, Ctrl+V no longer brings back files copied earlier in Wayfinder.
- **Cut cleanup** — after pasting a cut, the system clipboard is cleared, so neither Wayfinder nor another file manager tries to move the same files twice.

### Removed

- Unused `clap` and `open` dependencies.

## 2.3.0

### New features

- **Font zoom** — Ctrl+Plus/Equal to zoom in, Ctrl+Minus to zoom out, Ctrl+0 to reset. Range 50%–300%. Persisted across sessions. Announced to screen reader.
- **Breadcrumb path bar** — clickable path segments in the header bar replace the read-only location entry. Each segment navigates to that directory. Auto-scrolls to show the deepest segment. Special URIs (Bin, Recent) show a static label. F4 or header bar button to show/hide, persisted across sessions. Announces show/hide state.
- **Recent files** — "Recent" entry in the sidebar shows up to 50 recently opened local files via GTK RecentManager.
- **Batch rename** (Ctrl+Shift+F2) — select multiple files with Space, then batch rename with find/replace. Live preview shows old → new names. Accessible labels on all controls.
- **Replace/Skip/Rename conflict dialog** — when pasting files that already exist at the destination, a dialog offers Skip, Rename (keep both), or Replace. Replace uses OVERWRITE flag throughout including recursive directory copies.
- **Permission editor** — the Properties dialog now has an interactive 3×3 checkbox grid (Owner/Group/Others × Read/Write/Execute) with bidirectional octal entry and an Apply button. Preserves file type bits.
- **Session restore** — all open windows and their directories are saved on exit and restored on next launch. Ctrl+N windows are included.

### FUSE / rclone mount support

- **FUSE mount detection** — reads `/proc/mounts` to detect rclone, sshfs, and other FUSE filesystems.
- **Async file monitor** — on FUSE paths, file monitor events use a 5-second cancellable timeout instead of blocking the main thread.
- **Folder sizes skipped on FUSE** — directories show "—" instead of triggering recursive stat calls that would hang.
- **Trash fallback** — when trash is not supported (e.g. rclone mounts), offers a confirmation dialog to delete permanently.
- **Cross-device directory move** — moving a directory to a FUSE mount now falls back to recursive copy + delete instead of failing with "not supported".
- **Directory copy to FUSE** — `make_directory_with_parents` handles "already exists" errors gracefully.

### Accessibility

- **Action progress dialogs** — Extract Here, Compress, GPG actions, and all other custom actions now show a progress dialog with a pulsing bar while running. The dialog closes automatically on completion and announces success or failure via screen reader.
- **Paste completion announcements** — copy and move operations now announce "Copied filename" or "Moved filename" when each file completes.
- **Multi-file compress** — selecting multiple files with Space and choosing Compress adds all selected files to a single archive. The compress dialog now receives all selected files, not just the focused one.

### Fixed

- **Single-file archive extraction** — archives containing a single file no longer dump it into the parent directory; a subfolder named after the archive is created.
- **Directory replace overwrites correctly** — choosing "Replace" for an existing directory now deletes the old directory before copying, instead of silently merging contents.
- **FUSE permanent delete reloads directory** — after confirming permanent deletion on a FUSE mount, the directory listing now refreshes immediately.
- **Compress dialog default name** — now uses the file stem (without extension) as the default archive name instead of the full filename.
- **Archive extraction folder name** — dotted filenames like "R.A. Salvatore.7z" now extract to a correctly named folder instead of truncating at the first dot.
- **Undo trash accuracy** — undo buffer now only stores paths that were actually trashed, not files that failed.
- **Sidebar toggle** — changed from Ctrl+Shift+S to F3 for reliability across keyboard layouts. Now announces "Sidebar shown/hidden" via screen reader. Button and shortcut stay in sync.
- **Concurrent paste guard** — pressing Ctrl+V while a paste is already in progress now announces "Paste already in progress" instead of spawning duplicate file operations. The guard clears automatically when all files finish (including on error or cancellation).

### Code quality

- **Empty Bin fix** — `empty_trash` now uses recursive deletion so trashed directories are properly removed, not just empty files.
- **Non-local file safety** — copy/move progress dialog gracefully handles non-local GIO files (e.g. trash:// URIs) instead of panicking.
- **Portal runtime safety** — portal D-Bus response serialization no longer panics on failure.
- **Range selection cleanup** — replaced `unwrap()` on range anchor with `if let Some` pattern in both list and grid views.
- **Dead code removal** — removed unused variable bindings in location dialog setup.
- **Redundant clone removal** — eliminated unnecessary `.clone()` calls in drop targets, context menu key handlers, batch rename, and list view builder.
- **Duplicate builder property** — removed duplicated `show_column_separators(true)` in list view construction.
- **Modern format strings** — inlined all format arguments across the codebase (`format!("{var}")` instead of `format!("{}", var)`).
- **Completion callback reliability** — `copy_with_progress` and `move_with_progress` now always fire their completion callback on error, cancellation, and early return paths, not just on success. Prevents callers' cleanup logic from being skipped.
- **Idiomatic patterns** — replaced `while c.is_some() { c.unwrap()... }` with `while let Some(widget)` in sidebar, unnested or-patterns.
- **Sort performance** — name sorting now uses the cached lowercase `search_string` instead of allocating a new `to_lowercase()` string on every comparison, eliminating ~20,000 allocations per sort on a 1,000-file directory.

### Removed

- **Dual pane mode** — removed due to unreliable GTK keyboard shortcut handling with Shift+number keys across keyboard layouts.

---

## 2.2.3

### Accessibility

- **Empty folder announcements** — opening an empty folder announces "Opened X, folder is empty". Opening an empty Bin announces "Opened Bin, Bin is empty". Trashing or deleting the last file announces "folder is now empty" in the message.

---

## 2.2.2

### Fixed

- **Ctrl+A select all** now works in both list and grid views. Previously intercepted by GTK's built-in handler before reaching Wayfinder's selection logic.
- **Shift+Delete multi-file permanent delete** now works with Space-selected files. Previously only deleted the single focused file.
- **Extract Here** for zip files with no top-level folder now creates a subfolder named after the archive instead of dumping files loose into the parent directory.
- **Escape key** now closes the Rename and Create Folder dialogs (previously required clicking Cancel).
- **Focus restoration** after closing the Rename, Create Folder, and Keyboard Shortcuts dialogs — focus returns to the file list instead of getting lost.
- **Directory reload** after rename and create folder operations as a fallback when the file monitor doesn't catch the change.
- **Crash prevention** — replaced unsafe `.unwrap()` calls in file copy/move operations with safe early returns for edge cases (files with no basename, unmounted paths).

---

## 2.2.1

### Fixed

- Restoring a file from the Bin now refreshes the trash listing immediately, with focus moving to the nearest remaining item.
- Trash failures (e.g. permission denied) are now announced via screen reader before any focus changes, so the error is heard first.

### Code quality

- Resolved all clippy warnings (unused imports, boolean simplification, const thread_local, identical branches).
- Updated dependencies to latest compatible patch versions.

---

## 2.2.0

### Context menu

- **Copy Path** — copies the full file path to the system clipboard for pasting into terminals, text editors, etc. Announces "Copied path: /full/path".
- **Copy Name** — copies just the filename to the system clipboard. Announces "Copied name: filename".

### Undo

- **Undo Trash (Ctrl+Z)** — restores the most recently trashed file(s). Searches the Bin by original path, restores to the original location, and reloads the directory. Announces what was restored.

### Terminal

- **Open Terminal Here (Ctrl+`)** — opens a terminal emulator in the current directory. Auto-detects foot, alacritty, gnome-terminal, or konsole.

### Accessibility

- **Sort order announcements** — clicking a column header now announces "Sorted by Name, ascending" (or descending) via the screen reader.

---

## 2.1.0

### Clipboard

- **Cross-window copy/paste** — Ctrl+C/X/V now uses a global clipboard shared across all Wayfinder windows. Copy in one window, paste in another.
- **Window-local clipboard** — Ctrl+Shift+C/X/V for copy/cut/paste scoped to the current window only. Announcements say "(this window)" to distinguish.

### Keyboard shortcuts

- **Backspace** goes to parent directory (same as Alt+Up).
- **Ctrl+?** (Ctrl+Shift+/) opens a Keyboard Shortcuts window listing every shortcut organised by category: Navigation, File Operations, View, and General. Each entry is read by Orca as "Description: Key".
- **Ctrl+Shift+R** now opens File System (previously Ctrl+Shift+C, which is now window-local copy).

### Sidebar

- **Bookmark reordering** — Ctrl+Up/Down reorders bookmarks directly in the sidebar. Announces "Moved above/below {name}" and "Already at top/bottom" at boundaries. Changes persist to the bookmarks file.

### Fixed

- Backspace shortcut was listed in documentation but never registered.

---

## 2.0.0

A major feature release focused on extensibility, integration, and accessibility.

### Sidebar

- **Bookmarks** — Ctrl+D to bookmark the current folder. Delete key to remove. Compatible with `~/.config/gtk-3.0/bookmarks` (Nautilus/GTK format).
- **Edit Sidebar** — right-click or Menu key to open the editor. Toggle places on/off, reorder with Ctrl+Up/Down and Ctrl+Shift+Home/End. Full screen reader announcements ("Moved above Documents", "Already at top").
- **Volume management** — mounted and unmounted volumes appear in the sidebar with eject buttons. Volumes auto-update via GIO VolumeMonitor signals. Click to mount, eject button or right-click to unmount.

### Custom actions

- **Actions system** — context menu actions loaded from `.desktop` files in `~/.local/share/wayfinder/actions/`, `/usr/share/file-manager/actions/` (FMA standard), and `/usr/share/wayfinder/actions/`.
- **TryExec** — actions are hidden if their command is not installed, so bundled defaults work on any system.
- **Bundled actions** — Extract Here (file-roller/ark/bsdtar), Compress (with format picker: zip, tar.gz, tar.xz, tar.zst, 7z), GPG encrypt/decrypt/verify, SHA-256 checksum, and Open Terminal Here (foot/alacritty/gnome-terminal/konsole).
- **Nautilus scripts** — executable scripts in `~/.local/share/nautilus/scripts/` appear in the context menu with Nautilus-compatible environment variables.
- **Compress dialog** — pick archive name and format from a dialog. Detected formats based on installed tools.

### Integration

- **D-Bus FileManager1** — Wayfinder registers `org.freedesktop.FileManager1` on the session bus. External apps can call ShowFolders, ShowItems, and ShowItemProperties to open Wayfinder at specific paths.
- **Drag and drop** — files can be dragged from Wayfinder to other apps (copy/move). Drop files into Wayfinder to copy them to the current directory.
- **Tab path completion** — press Tab in the location bar to auto-complete paths, with `~` expansion and hidden file awareness.

### File operations

- **Recursive directory copy** — copying folders now works across filesystems (previously only single files were supported by GIO).
- **Reload after copy/move** — the directory listing refreshes automatically when a copy or move operation completes.
- **Focus after delete** — deleting or trashing a file focuses the item above it instead of jumping back to the top.

### Navigation

- **Type-ahead search** — start typing in the file list to jump to the first matching file. Buffer resets after 800ms. Clears on directory change.

### Accessibility

- **Context menu** — fully rebuilt as an accessible popover with `Menu` and `MenuItem` roles, arrow key navigation with wrapping, and Right/Left for submenu entry/exit.
- **Properties dialog** — accessible label ("Properties for {name}").
- **Progress dialogs** — accessible labels describing the operation.
- **Location bar** — autocomplete hint for screen readers.
- **Sidebar keyboard** — Menu key and Shift+F10 open context menus on sidebar items. Bookmark right-click shows "Remove Bookmark" and "Edit Sidebar".

### Internals

- **Symlink-safe folder sizes** — `symlink_metadata()` prevents following symlinks. Depth guard at 100 prevents infinite recursion.
- **Atomic state persistence** — window size and sort state use load-modify-save instead of separate writes.
- **O(1) folder size updates** — HashMap lookup instead of linear scan when updating directory sizes.

---

## 1.2.1

### Fixed

- Context menu items now actually activate. Replaced GMenuModel-based PopoverMenu (whose actions silently failed to resolve) with a manual Popover built from Button widgets.
- Use `GdkAppLaunchContext` instead of `NONE` when launching apps, so child processes inherit the Wayland display environment.

---

## 1.2.0

### Added

- Multi-file selection with Space to toggle and Shift+Space for range selection.
- Location dialog (Ctrl+L) for typing a path directly.
- Portal backend (`wayfinder-portal`) for XDG Desktop Portal file chooser integration.
- Select All (Ctrl+A).

---

## 1.1.0

### Added

- Asynchronous folder size calculation in a background thread.
- Sortable column headers (Name, Size, Modified, Kind) with saved sort state.
- Sidebar with Home, Desktop, Documents, Downloads, Music, Pictures, Videos, File System, and Bin.
- Sidebar toggle (Ctrl+B) with saved visibility state.

### Fixed

- Various stability and performance improvements.

---

## 1.0.0

Initial release.

- GTK4 file manager with list and grid views.
- Full keyboard navigation designed for screen reader users.
- Orca accessibility with `announce()` for state changes.
- File operations: copy, cut, paste, rename, trash, delete, properties.
- Navigation: back, forward, up, path bar.
- Hidden file toggle (Ctrl+H).
- Search (Ctrl+F).
- Per-file app associations via Properties dialog.
