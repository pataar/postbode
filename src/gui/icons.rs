//! The Phosphor icons the window uses, by meaning rather than by glyph name.
#![expect(
    dead_code,
    reason = "views adopt these icons one redesign task at a time"
)]
use egui_phosphor::regular as ph;

pub(crate) const ACTIVITY: &str = ph::PULSE;
pub(crate) const ARCHIVE: &str = ph::ARCHIVE;
pub(crate) const BACKUPS: &str = ph::CLOCK_COUNTER_CLOCKWISE;
pub(crate) const CARET_DOWN: &str = ph::CARET_DOWN;
pub(crate) const CARET_RIGHT: &str = ph::CARET_RIGHT;
pub(crate) const CHECK: &str = ph::CHECK;
pub(crate) const DRAFTS: &str = ph::FILE_DASHED;
pub(crate) const FLAG: &str = ph::FLAG;
pub(crate) const FOLDER: &str = ph::FOLDER;
pub(crate) const INBOX: &str = ph::TRAY;
pub(crate) const JUNK: &str = ph::WARNING_OCTAGON;
pub(crate) const MARK_UNREAD: &str = ph::ENVELOPE_SIMPLE;
pub(crate) const MOVE: &str = ph::FOLDER_SIMPLE_DASHED;
pub(crate) const RULES: &str = ph::FUNNEL;
pub(crate) const SEARCH: &str = ph::MAGNIFYING_GLASS;
pub(crate) const SENT: &str = ph::PAPER_PLANE_TILT;
pub(crate) const SYNC: &str = ph::ARROWS_CLOCKWISE;
pub(crate) const TRASH: &str = ph::TRASH;

/// The icon for a folder: its special use, INBOX by name, else a plain folder.
pub(crate) fn for_special_use(special_use: Option<&str>, name: &str) -> &'static str {
    if name.eq_ignore_ascii_case("INBOX") {
        return INBOX;
    }
    match special_use {
        Some("Archive") => ARCHIVE,
        Some("Drafts") => DRAFTS,
        Some("Junk") => JUNK,
        Some("Sent") => SENT,
        Some("Trash") => TRASH,
        _ => FOLDER,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn special_folders_get_their_icon_and_others_a_folder() {
        assert_eq!(for_special_use(None, "INBOX"), INBOX);
        assert_eq!(for_special_use(None, "inbox"), INBOX);
        assert_eq!(for_special_use(Some("Archive"), "All Mail"), ARCHIVE);
        assert_eq!(for_special_use(Some("Drafts"), "Drafts"), DRAFTS);
        assert_eq!(for_special_use(Some("Junk"), "Spam"), JUNK);
        assert_eq!(for_special_use(Some("Sent"), "Sent Items"), SENT);
        assert_eq!(for_special_use(Some("Trash"), "Deleted"), TRASH);
        assert_eq!(for_special_use(None, "Receipts"), FOLDER);
    }
}
