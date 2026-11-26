use anyhow::{bail, Context, Result};
use std::{
    fs,
    path::Path,
    process::{Command, Stdio},
};

#[derive(Debug)]
pub struct ArchiveEntry {
    pub name: String,
    pub is_dir: bool,
}

/// Inspect a ZIP before touching the filesystem. zipinfo exposes Unix file types, so
/// archives containing symlinks or special files are rejected before extraction.
pub fn inspect_archive(zip: &Path) -> Result<Vec<ArchiveEntry>> {
    let names = Command::new("/usr/bin/zipinfo")
        .arg("-1")
        .arg(zip)
        .output()?;
    if !names.status.success() {
        bail!("It isn't a readable zip file.");
    }
    let names = String::from_utf8(names.stdout)
        .context("ZIP contains filenames that aren't valid UTF-8")?;
    let details = Command::new("/usr/bin/zipinfo")
        .arg("-s")
        .arg(zip)
        .output()?;
    if !details.status.success() {
        bail!("The ZIP entry details couldn't be read.");
    }
    let details =
        String::from_utf8(details.stdout).context("ZIP entry details aren't valid UTF-8")?;
    let mut kinds = Vec::new();
    for line in details.lines() {
        let b = line.as_bytes();
        if b.len() < 10 || !matches!(b[0], b'-' | b'd' | b'l' | b'b' | b'c' | b'p' | b's' | b'?') {
            continue;
        }
        if !b[1..10].iter().all(|c| {
            matches!(
                *c,
                b'r' | b'w' | b'x' | b's' | b'S' | b't' | b'T' | b'-' | b'?'
            )
        }) {
            continue;
        }
        kinds.push(b[0]);
    }
    validate_archive_entries(&names, &kinds)
}

fn validate_archive_entries(names: &str, kinds: &[u8]) -> Result<Vec<ArchiveEntry>> {
    let names: Vec<&str> = names.lines().collect();
    if names.is_empty() || names.len() != kinds.len() {
        bail!("The ZIP has ambiguous or unsupported entry names.");
    }

    let mut entries = Vec::with_capacity(names.len());
    let mut seen = std::collections::HashSet::new();
    let mut has_data = false;
    let mut has_conf = false;
    for (name, kind) in names.into_iter().zip(kinds.iter().copied()) {
        let is_dir = name.ends_with('/');
        if (is_dir && kind != b'd') || (!is_dir && kind != b'-') {
            bail!("The ZIP contains a symbolic link or a non-regular file: {name}");
        }
        if name.contains('\\') || name.chars().any(|c| matches!(c, '*' | '?' | '[' | ']')) {
            bail!("The ZIP contains an unsupported entry name: {name}");
        }
        let path = name.strip_suffix('/').unwrap_or(name);
        let parts: Vec<&str> = path.split('/').collect();
        if parts
            .iter()
            .any(|part| part.is_empty() || *part == "." || *part == "..")
        {
            bail!("The ZIP contains an unsafe entry path: {name}");
        }
        let metadata = path == crate::snapshot::MANIFEST || path == "SNAPSHOT.txt";
        let archive_root = parts.first().copied().unwrap_or_default();
        let in_wiki_tree = archive_root == "data" || archive_root == "conf";
        if !(metadata || in_wiki_tree)
            || (metadata && (is_dir || parts.len() != 1))
            || (in_wiki_tree && parts.len() < 2 && !is_dir)
            || (in_wiki_tree && parts.len() == 1 && !is_dir)
        {
            bail!("It doesn't look like an ArchiveWiki backup (unexpected entry: {name}).");
        }
        if !seen.insert(path.to_string()) {
            bail!("The ZIP contains a duplicate entry: {name}");
        }
        has_data |= archive_root == "data";
        has_conf |= archive_root == "conf";
        entries.push(ArchiveEntry {
            name: name.to_string(),
            is_dir,
        });
    }
    if !has_data || !has_conf {
        bail!("The ZIP must contain both wiki data and configuration.");
    }
    Ok(entries)
}

pub fn validate_backup_zip(zip: &Path) -> Result<()> {
    inspect_archive(zip).map(|_| ())
}

/// Extract only prevalidated regular files, one at a time, into a new staging folder.
pub fn extract_archive(zip: &Path, stage: &Path, entries: &[ArchiveEntry]) -> Result<()> {
    let _ = fs::remove_dir_all(stage);
    fs::create_dir_all(stage)?;
    for entry in entries {
        let relative = entry.name.strip_suffix('/').unwrap_or(&entry.name);
        let target = stage.join(relative);
        if entry.is_dir {
            fs::create_dir_all(&target)?;
            continue;
        }
        let parent = target
            .parent()
            .context("archive entry has no parent folder")?;
        fs::create_dir_all(parent)?;
        let file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&target)
            .with_context(|| format!("creating staged file {}", target.display()))?;
        let status = Command::new("/usr/bin/unzip")
            .arg("-p")
            .arg(zip)
            .arg(&entry.name)
            .stdout(Stdio::from(file))
            .stderr(Stdio::null())
            .status()?;
        if !status.success() {
            bail!("The ZIP entry couldn't be extracted: {}", entry.name);
        }
    }
    Ok(())
}

/// Replace data and configuration as one operation; restore both previous folders if a rename fails.
pub fn install_staged_restore(wiki: &Path, stage: &Path, previous: &Path) -> Result<()> {
    let _ = fs::remove_dir_all(previous);
    fs::create_dir_all(previous)?;
    let mut saved: Vec<&str> = Vec::new();
    for top in ["data", "conf"] {
        let current = wiki.join(top);
        if fs::symlink_metadata(&current).is_ok() {
            if let Err(e) = fs::rename(&current, previous.join(top)) {
                for old in saved.iter().rev() {
                    let _ = fs::rename(previous.join(*old), wiki.join(*old));
                }
                return Err(e).with_context(|| format!("setting aside the current {top} folder"));
            }
            saved.push(top);
        }
    }
    let install_result = (|| -> Result<()> {
        for top in ["data", "conf"] {
            fs::rename(stage.join(top), wiki.join(top))
                .with_context(|| format!("installing restored {top}"))?;
        }
        Ok(())
    })();
    if let Err(e) = install_result {
        for top in ["data", "conf"] {
            let _ = fs::remove_dir_all(wiki.join(top));
        }
        for old in saved.iter().rev() {
            let _ = fs::rename(previous.join(*old), wiki.join(*old));
        }
        return Err(e).context("restore failed; the previous wiki folders were put back");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("archivewiki-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).unwrap();
        path
    }

    #[test]
    fn accepts_regular_data_and_configuration_entries() {
        let entries =
            validate_archive_entries("data/pages/start.txt\nconf/local.php\n", b"--").unwrap();
        assert_eq!(entries.len(), 2);
        assert!(!entries[0].is_dir);
    }

    #[test]
    fn rejects_traversal_and_duplicate_paths() {
        assert!(validate_archive_entries("data/pages/../escape\nconf/local.php\n", b"--").is_err());
        assert!(validate_archive_entries(
            "data/pages/start.txt\ndata/pages/start.txt\nconf/local.php\n",
            b"---",
        )
        .is_err());
    }

    #[test]
    fn rejects_symlinks_and_incomplete_backups() {
        assert!(validate_archive_entries("data/pages/link\nconf/local.php\n", b"l-").is_err());
        assert!(validate_archive_entries("data/pages/start.txt\n", b"-").is_err());
    }

    #[test]
    fn restores_previous_folders_when_staged_install_fails() {
        let root = temp_dir("restore-rollback");
        let wiki = root.join("wiki");
        let stage = root.join("stage");
        let previous = root.join("previous");
        fs::create_dir_all(wiki.join("data")).unwrap();
        fs::create_dir_all(wiki.join("conf")).unwrap();
        fs::write(wiki.join("data/original.txt"), "old data").unwrap();
        fs::write(wiki.join("conf/original.php"), "old config").unwrap();
        fs::create_dir_all(stage.join("data")).unwrap();
        fs::write(stage.join("data/replacement.txt"), "new data").unwrap();

        assert!(install_staged_restore(&wiki, &stage, &previous).is_err());
        assert_eq!(
            fs::read_to_string(wiki.join("data/original.txt")).unwrap(),
            "old data"
        );
        assert_eq!(
            fs::read_to_string(wiki.join("conf/original.php")).unwrap(),
            "old config"
        );
        assert!(!wiki.join("data/replacement.txt").exists());
    }
}
