//! Checksum manifests for archive snapshots. Pure std + sha2, so it can be tested anywhere.
//! The manifest uses the same format as `shasum -a 256` / `sha256sum` ("<hex>  <path>"), so a snapshot can be
//! verified without ArchiveWiki:  cd <unzipped snapshot> && shasum -a 256 -c MANIFEST.sha256

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs, io,
    path::{Component, Path, PathBuf},
};

pub const MANIFEST: &str = "MANIFEST.sha256";
pub const TOP_DIRS: [&str; 2] = ["data", "conf"];

pub fn sha256_file(p: &Path) -> Result<String> {
    let mut f = fs::File::open(p).with_context(|| format!("opening {}", p.display()))?;
    let mut h = Sha256::new();
    io::copy(&mut f, &mut h)?;
    Ok(format!("{:x}", h.finalize()))
}

pub fn verify_sha256(p: &Path, expected: &str) -> Result<()> {
    let expected = expected.trim().to_ascii_lowercase();
    if expected.len() != 64 || !expected.bytes().all(|b| b.is_ascii_hexdigit()) {
        anyhow::bail!("expected SHA-256 must contain exactly 64 hexadecimal characters");
    }
    let got = sha256_file(p)?;
    if got != expected {
        anyhow::bail!("SHA-256 mismatch: expected {expected}, got {got}");
    }
    Ok(())
}

fn walk(root: &Path, rel: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for e in fs::read_dir(root.join(rel))? {
        let e = e?;
        let r = rel.join(e.file_name());
        let ft = e.file_type()?; // does not follow symlinks, so links can be rejected safely
        if ft.is_dir() {
            walk(root, &r, out)?;
        } else if ft.is_file() {
            out.push(r);
        } else if ft.is_symlink() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("snapshot contains a symbolic link: {}", r.display()),
            ));
        }
    }
    Ok(())
}

/// All regular files under root/data and root/conf, relative to root, sorted.
pub fn list_files(root: &Path) -> io::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    for top in TOP_DIRS {
        let path = root.join(top);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_dir() => {
                walk(root, Path::new(top), &mut files)?;
            }
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("snapshot contains a symbolic link: {}", path.display()),
                ));
            }
            Ok(_) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    files.sort();
    Ok(files)
}

fn rel_str(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/")
}

/// Hash every file under data/ and conf/ and write root/MANIFEST.sha256. Returns the number of files.
pub fn write_manifest(root: &Path) -> Result<usize> {
    let files = list_files(root)?;
    let mut text = String::new();
    for f in &files {
        let h = sha256_file(&root.join(f))?;
        text.push_str(&format!("{h}  {}\n", rel_str(f)));
    }
    fs::write(root.join(MANIFEST), text)?;
    Ok(files.len())
}

#[derive(Debug, Default)]
pub struct Verification {
    pub checked: usize,
    pub changed: Vec<String>,
    pub missing: Vec<String>,
    pub extra: Vec<String>,
}

impl Verification {
    pub fn ok(&self) -> bool {
        self.changed.is_empty() && self.missing.is_empty() && self.extra.is_empty()
    }
}

/// Recompute every hash under root and compare with root/MANIFEST.sha256.
pub fn verify_tree(root: &Path) -> Result<Verification> {
    let manifest = root.join(MANIFEST);
    let metadata = fs::symlink_metadata(&manifest).context("this folder has no MANIFEST.sha256")?;
    if !metadata.file_type().is_file() {
        anyhow::bail!("MANIFEST.sha256 must be a regular file");
    }
    let text = fs::read_to_string(&manifest).context("reading MANIFEST.sha256")?;
    let mut expected: BTreeMap<String, String> = BTreeMap::new();
    for line in text.lines() {
        let (hash, path) = line
            .split_once("  ")
            .context("invalid entry in MANIFEST.sha256")?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            anyhow::bail!("invalid SHA-256 value in MANIFEST.sha256");
        }
        let rel = Path::new(path);
        let mut components = rel.components();
        let top = components.next();
        if !matches!(top, Some(Component::Normal(c)) if c == "data" || c == "conf")
            || components.clone().next().is_none()
            || components.any(|c| !matches!(c, Component::Normal(_)))
            || path.contains('\\')
        {
            anyhow::bail!("unsafe path in MANIFEST.sha256: {path}");
        }
        if expected
            .insert(path.to_string(), hash.to_ascii_lowercase())
            .is_some()
        {
            anyhow::bail!("duplicate path in MANIFEST.sha256: {path}");
        }
    }
    let mut v = Verification::default();
    for (path, want) in &expected {
        let p = root.join(path);
        let metadata = match fs::symlink_metadata(&p) {
            Ok(meta) => meta,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                v.missing.push(path.clone());
                continue;
            }
            Err(e) => return Err(e).with_context(|| format!("checking {}", p.display())),
        };
        if !metadata.file_type().is_file() {
            v.missing.push(path.clone());
            continue;
        }
        v.checked += 1;
        if &sha256_file(&p)? != want {
            v.changed.push(path.clone());
        }
    }
    for f in list_files(root)? {
        let s = rel_str(&f);
        if !expected.contains_key(&s) {
            v.extra.push(s);
        }
    }
    Ok(v)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("aw-snap-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    fn sample(root: &Path) {
        fs::create_dir_all(root.join("data/pages/people")).unwrap();
        fs::create_dir_all(root.join("data/media")).unwrap();
        fs::create_dir_all(root.join("conf")).unwrap();
        fs::write(root.join("data/pages/start.txt"), "hello\n").unwrap();
        fs::write(
            root.join("data/pages/people/jane doe.txt"),
            "Jane — née Smith\n",
        )
        .unwrap();
        fs::write(root.join("data/media/blob.bin"), vec![7u8; 100_000]).unwrap();
        fs::write(root.join("conf/local.php"), "<?php\n").unwrap();
    }

    #[test]
    fn sha256_matches_known_value() {
        let d = temp("known");
        fs::write(d.join("abc.txt"), "abc").unwrap();
        assert_eq!(
            sha256_file(&d.join("abc.txt")).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn manifest_then_verify_is_clean() {
        let d = temp("clean");
        sample(&d);
        assert_eq!(write_manifest(&d).unwrap(), 4);
        let v = verify_tree(&d).unwrap();
        assert!(v.ok(), "{v:?}");
        assert_eq!(v.checked, 4);
    }

    #[test]
    fn detects_changed_missing_and_extra() {
        let d = temp("tamper");
        sample(&d);
        write_manifest(&d).unwrap();
        fs::write(d.join("data/pages/start.txt"), "hello!\n").unwrap();
        fs::remove_file(d.join("conf/local.php")).unwrap();
        fs::write(d.join("data/pages/new.txt"), "x").unwrap();
        let v = verify_tree(&d).unwrap();
        assert!(!v.ok());
        assert_eq!(v.changed, vec!["data/pages/start.txt"]);
        assert_eq!(v.missing, vec!["conf/local.php"]);
        assert_eq!(v.extra, vec!["data/pages/new.txt"]);
    }

    #[test]
    fn missing_manifest_is_an_error() {
        let d = temp("nomanifest");
        sample(&d);
        assert!(verify_tree(&d).is_err());
    }

    #[test]
    fn sha256_verification_rejects_a_mismatch() {
        let d = temp("checksum-mismatch");
        let p = d.join("artifact.bin");
        fs::write(&p, b"artifact").unwrap();
        assert!(verify_sha256(&p, &"0".repeat(64)).is_err());
    }
}
