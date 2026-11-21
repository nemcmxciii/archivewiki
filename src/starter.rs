//! Starter-content helpers (pure std, unit-tested): add missing starter files and retire the ones an earlier
//! version shipped, but only while they are still exactly as shipped, so anything you wrote is never touched.

use std::{
    fs,
    path::{Path, PathBuf},
};

fn norm(s: &str) -> String {
    s.replace("\r\n", "\n").trim_end().to_string()
}

/// Delete files from `legacy` (relative path, shipped text) that still match the shipped text exactly, then remove
/// namespace folders that became empty. Returns how many files were deleted.
pub fn retire_legacy(wiki: &Path, legacy: &[(&str, &str)]) -> usize {
    let pages = wiki.join("data").join("pages");
    let mut removed = 0;
    let mut dirs: Vec<PathBuf> = Vec::new();
    for (rel, shipped) in legacy {
        let p = wiki.join(rel);
        if let Ok(current) = fs::read_to_string(&p) {
            if norm(&current) == norm(shipped) && fs::remove_file(&p).is_ok() {
                removed += 1;
            }
        }
        if let Some(d) = p.parent() {
            if d != pages {
                dirs.push(d.to_path_buf());
            }
        }
    }
    dirs.sort();
    dirs.dedup();
    for d in dirs.iter().rev() {
        let _ = fs::remove_dir(d); // only succeeds when the folder is empty
    }
    removed
}

/// Replace shipped starter files only when their contents still match the previous shipped version.
/// Returns how many files were updated; customized or missing files are left alone.
pub fn replace_if_unchanged(wiki: &Path, updates: &[(&str, &str, &str)]) -> usize {
    let mut replaced = 0;
    for (rel, previous, current) in updates {
        let path = wiki.join(rel);
        let Ok(existing) = fs::read_to_string(&path) else { continue };
        if norm(&existing) == norm(previous) && fs::write(&path, current).is_ok() {
            replaced += 1;
        }
    }
    replaced
}

/// Write each (relative path, text) that doesn't exist yet. Returns how many files were written.
pub fn write_missing(wiki: &Path, files: &[(&str, &str)]) -> usize {
    let mut written = 0;
    for (rel, text) in files {
        let p = wiki.join(rel);
        if p.exists() {
            continue;
        }
        if let Some(dir) = p.parent() {
            if fs::create_dir_all(dir).is_err() {
                continue;
            }
        }
        if fs::write(&p, text).is_ok() {
            written += 1;
        }
    }
    written
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("aw-starter-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(p.join("data/pages")).unwrap();
        p
    }

    #[test]
    fn unedited_legacy_files_are_removed_and_empty_folders_tidied() {
        let w = temp("unedited");
        fs::create_dir_all(w.join("data/pages/inbox")).unwrap();
        fs::write(w.join("data/pages/inbox/start.txt"), "old inbox\n").unwrap();
        fs::write(w.join("data/pages/start.txt"), "old start\r\n").unwrap(); // CRLF + trailing newline differences are tolerated
        let legacy = [("data/pages/inbox/start.txt", "old inbox"), ("data/pages/start.txt", "old start\n")];
        assert_eq!(retire_legacy(&w, &legacy), 2);
        assert!(!w.join("data/pages/inbox").exists(), "empty namespace folder should go");
        assert!(w.join("data/pages").exists(), "data/pages itself must stay");
    }

    #[test]
    fn edited_legacy_files_and_their_folders_are_left_alone() {
        let w = temp("edited");
        fs::create_dir_all(w.join("data/pages/records")).unwrap();
        fs::write(w.join("data/pages/records/employment.txt"), "my real job history\n").unwrap();
        fs::write(w.join("data/pages/records/start.txt"), "shipped\n").unwrap();
        let legacy = [("data/pages/records/employment.txt", "shipped table"), ("data/pages/records/start.txt", "shipped")];
        assert_eq!(retire_legacy(&w, &legacy), 1); // only the untouched start page
        assert_eq!(fs::read_to_string(w.join("data/pages/records/employment.txt")).unwrap(), "my real job history\n");
        assert!(w.join("data/pages/records").exists());
    }

    #[test]
    fn write_missing_never_overwrites() {
        let w = temp("write");
        fs::write(w.join("data/pages/start.txt"), "mine").unwrap();
        let files = [("data/pages/start.txt", "theirs"), ("data/pages/new/a.txt", "a")];
        assert_eq!(write_missing(&w, &files), 1);
        assert_eq!(fs::read_to_string(w.join("data/pages/start.txt")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(w.join("data/pages/new/a.txt")).unwrap(), "a");
    }
}
