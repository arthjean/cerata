//! Conflict copies of sync services (US-093), recognized by name next to
//! their original note:
//!
//! - Syncthing: `<stem>.sync-conflict-<YYYYMMDD>-<HHMMSS>-<device>.<ext>`
//!   (docs.syncthing.net/users/syncing.html, "Conflicting Changes");
//! - Dropbox: `<stem> (<editor>'s conflicted copy <YYYY-MM-DD>).<ext>`, the
//!   editor's name and any later suffix optional
//!   (help.dropbox.com/organize/conflicted-copy).
//!
//! OneDrive's pattern is not recognized until verified (PRD, Open
//! Questions).

use std::path::{Path, PathBuf};

/// Which service made a copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Service {
    Syncthing,
    Dropbox,
}

/// The original's file name of a conflict copy named `name`.
pub fn original_name(name: &str) -> Option<(String, Service)> {
    syncthing(name)
        .map(|original| (original, Service::Syncthing))
        .or_else(|| dropbox(name).map(|original| (original, Service::Dropbox)))
}

fn digits(text: &str, count: usize) -> bool {
    text.len() == count && text.bytes().all(|b| b.is_ascii_digit())
}

fn syncthing(name: &str) -> Option<String> {
    const MARK: &str = ".sync-conflict-";
    let at = name.find(MARK)?;
    let (stem, rest) = (&name[..at], &name[at + MARK.len()..]);
    // `<date>-<time>-<device>` then the extension, if any.
    let (stamp, ext) = match rest.find('.') {
        Some(dot) => (&rest[..dot], &rest[dot..]),
        None => (rest, ""),
    };
    let mut parts = stamp.splitn(3, '-');
    let (date, time, device) = (parts.next()?, parts.next()?, parts.next()?);
    let device_ok = !device.is_empty() && device.bytes().all(|b| b.is_ascii_alphanumeric());
    (!stem.is_empty() && digits(date, 8) && digits(time, 6) && device_ok)
        .then(|| format!("{stem}{ext}"))
}

fn dropbox(name: &str) -> Option<String> {
    const MARK: &str = "conflicted copy ";
    let (body, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    let body = body.strip_suffix(')')?;
    // The last ` (` opening the copy's mark: a stem may hold parentheses,
    // and a later `(1)` may follow the date.
    let open = body
        .match_indices(" (")
        .map(|(at, _)| at)
        .filter(|at| body[at + 2..].contains(MARK))
        .last()?;
    let (stem, inside) = (&body[..open], &body[open + 2..]);
    let at = inside.find(MARK)?;
    let before = &inside[..at];
    // Nothing, or `<editor>'s ` (straight or curly apostrophe).
    let editor_ok = before.is_empty() || before.ends_with("'s ") || before.ends_with("\u{2019}s ");
    let date = inside[at + MARK.len()..].get(..10)?;
    let date_ok = date.len() == 10
        && digits(&date[..4], 4)
        && &date[4..5] == "-"
        && digits(&date[5..7], 2)
        && &date[7..8] == "-"
        && digits(&date[8..10], 2);
    (!stem.is_empty() && editor_ok && date_ok).then(|| format!("{stem}{ext}"))
}

/// The original note of the conflict copy at `path`, when it exists in the
/// same folder.
pub fn original_of(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let (original, _) = original_name(name)?;
    let original = path.with_file_name(original);
    original.is_file().then_some(original)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn syncthing_copies_are_recognized() {
        assert_eq!(
            original_name("Plan.sync-conflict-20261103-142501-7ZQ3K4M.md"),
            Some(("Plan.md".into(), Service::Syncthing))
        );
        assert_eq!(
            original_name("a.b.sync-conflict-20261103-142501-ABCDEFG.md").map(|o| o.0),
            Some("a.b.md".into())
        );
        for name in [
            "Plan.sync-conflict-2026113-142501-7ZQ3K4M.md",
            "Plan.sync-conflict-20261103-1425-7ZQ3K4M.md",
            "Plan.sync-conflict-20261103-142501-.md",
            ".sync-conflict-20261103-142501-ABC.md",
            "Plan.md",
        ] {
            assert_eq!(original_name(name), None, "{name}");
        }
    }

    #[test]
    fn dropbox_copies_are_recognized() {
        for (name, original) in [
            ("Plan (Arthur's conflicted copy 2026-11-03).md", "Plan.md"),
            (
                "Plan (Arthur\u{2019}s conflicted copy 2026-11-03).md",
                "Plan.md",
            ),
            ("Plan (conflicted copy 2026-11-03).md", "Plan.md"),
            (
                "Plan (Arthur's conflicted copy 2026-11-03 (1)).md",
                "Plan.md",
            ),
            (
                "Two words (Jo's conflicted copy 2026-01-31).markdown",
                "Two words.markdown",
            ),
        ] {
            assert_eq!(
                original_name(name),
                Some((original.into(), Service::Dropbox)),
                "{name}"
            );
        }
        for name in [
            "Plan (conflicted copy).md",
            "Plan (Arthur conflicted copy 2026-11-03).md",
            "Plan (Arthur's conflicted copy 26-11-03).md",
            "Plan (2).md",
            " (conflicted copy 2026-11-03).md",
        ] {
            assert_eq!(original_name(name), None, "{name}");
        }
    }

    #[test]
    fn a_copy_without_its_original_is_an_ordinary_note() {
        let dir = tempfile::TempDir::new().unwrap();
        let copy = dir.path().join("Plan (conflicted copy 2026-11-03).md");
        std::fs::write(&copy, "x").unwrap();
        assert_eq!(original_of(&copy), None);
        std::fs::write(dir.path().join("Plan.md"), "y").unwrap();
        assert_eq!(original_of(&copy), Some(dir.path().join("Plan.md")));
    }
}
