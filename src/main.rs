#[cfg(not(target_os = "macos"))]
compile_error!("ArchiveWiki targets macOS only.");

use anyhow::{anyhow, bail, Context, Result};
use muda::{
    accelerator::{Accelerator, Code, Modifiers},
    CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem, Submenu,
};
use std::{
    fs,
    net::{SocketAddr, TcpListener, TcpStream},
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant, SystemTime},
};
use tao::{
    dpi::LogicalSize,
    event::{Event, StartCause, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy},
    window::WindowBuilder,
};
use wry::WebViewBuilder;

const APP_NAME: &str = "ArchiveWiki";
/// Fixed preferred port keeps the webview origin (cookies, localStorage) stable across launches.
const PREFERRED_PORT: u16 = 47631;
const ROUTER_PHP: &str = include_str!("router.php");
const LOCAL_SEED: &str = include_str!("local.php.seed");

// Update source. VERSION on the stable branch is tiny and reliable; the tarball is the official release build.
const VERSION_URL: &str = "https://raw.githubusercontent.com/dokuwiki/dokuwiki/stable/VERSION";
const DOWNLOAD_URL: &str = "https://download.dokuwiki.org/src/dokuwiki/dokuwiki-stable.tgz";
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
// PHP builds come from this project's own GitHub Releases (made by the "Build PHP" workflow, which includes
// pdo_sqlite). Updates stay within the installed minor line (8.3.x). The repository must be public.
const PHP_REPO: &str = "nemcmxciii/archivewiki";

fn php_list_url() -> String {
    format!("https://api.github.com/repos/{PHP_REPO}/releases?per_page=30")
}

fn php_download_url(ver: &str) -> String {
    format!("https://github.com/{PHP_REPO}/releases/download/php-{ver}/php-{ver}-cli-macos-aarch64.tar.gz")
}

#[derive(Debug)]
enum UserEvent {
    Menu(MenuEvent),
    /// A verified PHP build has been downloaded; main thread swaps it in (with automatic rollback).
    ApplyPhp { staged: PathBuf, version: String },
    /// A verified update has been downloaded + backed up; main thread applies it.
    ApplyUpdate { staging: PathBuf, version: String },
}

#[derive(Clone)]
struct Layout {
    php: PathBuf,        // installed copy (Application Support/runtime/php) — what actually runs
    bundle_php: PathBuf, // pristine copy inside the .app
    runtime: PathBuf,
    bundle_wiki: PathBuf,
    wiki: PathBuf,
    router: PathBuf,
    log: PathBuf,
    pidfile: PathBuf,
    updates: PathBuf,
    rollback: PathBuf,
    support: PathBuf,
}

impl Layout {
    fn discover() -> Result<Self> {
        let res = resource_dir()?;
        let support = dirs::data_dir()
            .ok_or_else(|| anyhow!("Cannot locate ~/Library/Application Support"))?
            .join(APP_NAME);
        fs::create_dir_all(&support)?;
        Ok(Self {
            php: support.join("runtime").join("php"),
            bundle_php: res.join("php"),
            runtime: support.join("runtime"),
            bundle_wiki: res.join("dokuwiki"),
            wiki: support.join("wiki"),
            router: support.join("router.php"),
            log: support.join("php-server.log"),
            pidfile: support.join("php.pid"),
            updates: support.join("updates"),
            rollback: support.join("wiki-rollback"),
            support,
        })
    }
}

fn resource_dir() -> Result<PathBuf> {
    if let Ok(p) = std::env::var("DWA_RESOURCES") {
        return Ok(p.into());
    }
    let exe = std::env::current_exe()?;
    if let Some(contents) = exe.parent().and_then(|p| p.parent()) {
        let r = contents.join("Resources");
        if r.join("php").exists() {
            return Ok(r);
        }
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources");
    if dev.join("php").exists() {
        return Ok(dev);
    }
    bail!("Bundled PHP runtime not found. Run scripts/fetch-deps.sh first.")
}

// ---------------------------------------------------------------- wiki files

/// A binary copied out of a downloaded app can carry the "downloaded from the internet" flag and an
/// invalid-for-this-location signature, and macOS then kills it on launch (SIGKILL). Clear the flag and
/// re-sign it ad hoc, locally, so it runs like a binary you built yourself.
fn prepare_binary(p: &Path) {
    let _ = Command::new("/usr/bin/xattr").arg("-c").arg(p).status();
    let _ = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(p).status();
}

/// Keep a writable copy of PHP in Application Support so it can be updated without touching the signed .app.
/// The bundled copy is (re)installed on first run, when the installed copy won't run, or when it is newer.
fn sync_php(bundled: &Path, installed: &Path) -> Result<()> {
    if !bundled.exists() {
        bail!("Bundled PHP missing at {}", bundled.display());
    }
    let installed_ver = if installed.exists() { php_version(installed) } else { None };
    let need = match installed_ver {
        None => true, // missing, or present but won't start
        Some(iv) => match (php_version(bundled).and_then(|v| parse_ver(&v)), parse_ver(&iv)) {
            (Some(b), Some(i)) => b > i,
            _ => false,
        },
    };
    if need {
        if let Some(dir) = installed.parent() {
            fs::create_dir_all(dir)?;
        }
        let _ = fs::remove_file(installed);
        fs::copy(bundled, installed).context("installing bundled PHP")?;
        fs::set_permissions(installed, fs::Permissions::from_mode(0o755))?;
        prepare_binary(installed);
        if php_version(installed).is_none() {
            bail!("The bundled PHP could not be started on this Mac (macOS refused to run it).");
        }
    }
    Ok(())
}

/// Install DokuWiki into the writable app-support dir on first run, and re-apply the bundled copy only
/// if it is NEWER than what's installed (so an in-app update is never overwritten by an older bundle).
/// Existing files under data/ (your pages & media) are never overwritten. User config
/// (conf/local.php, users.auth.php, ...) isn't in the release, so it is never touched.
fn sync_wiki(bundle: &Path, wiki: &Path) -> Result<()> {
    if !bundle.join("doku.php").exists() {
        bail!("Bundled DokuWiki missing at {}", bundle.display());
    }
    let fresh = !wiki.join("doku.php").exists();
    let bundled = fs::read_to_string(bundle.join("VERSION")).unwrap_or_default();
    let installed = installed_version(wiki);
    if fresh || is_newer(&bundled, &installed) {
        copy_tree(bundle, wiki, Path::new(""), &[])?;
        if !fresh {
            apply_deleted_list(bundle, wiki);
        }
    }
    let _ = fs::remove_file(wiki.join("install.php"));
    let local = wiki.join("conf").join("local.php");
    if !local.exists() {
        fs::write(&local, LOCAL_SEED)?;
    }
    Ok(())
}

/// Overlay src onto dst. Existing files under data/ are never overwritten.
/// `exclude_top` skips named top-level entries (used to snapshot everything except data/).
fn copy_tree(src: &Path, dst: &Path, rel: &Path, exclude_top: &[&str]) -> Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let name = entry.file_name();
        if rel.as_os_str().is_empty() && exclude_top.iter().any(|x| name == *x) {
            continue;
        }
        let (s, d, r) = (entry.path(), dst.join(&name), rel.join(&name));
        let ft = entry.file_type()?;
        if ft.is_dir() {
            copy_tree(&s, &d, &r, exclude_top)?;
        } else if ft.is_file() {
            if r.starts_with("data") && d.exists() {
                continue; // never clobber user content
            }
            fs::copy(&s, &d).with_context(|| format!("copying {}", s.display()))?;
        }
    }
    Ok(())
}

/// DokuWiki ships data/deleted.files: paths removed from the project in past releases.
/// Remove them (as DokuWiki's own upgrade instructions say), never touching data/ or conf/.
fn apply_deleted_list(release: &Path, wiki: &Path) {
    let Ok(txt) = fs::read_to_string(release.join("data").join("deleted.files")) else { return };
    for line in txt.lines() {
        let p = line.trim();
        if p.is_empty() || p.starts_with('#') || p.contains("..") || p.starts_with('/') {
            continue;
        }
        if p.starts_with("data") || p.starts_with("conf") {
            continue;
        }
        let t = wiki.join(p);
        if t.is_dir() {
            let _ = fs::remove_dir_all(&t);
        } else if t.exists() {
            let _ = fs::remove_file(&t);
        }
    }
}

// ---------------------------------------------------------------- updates

fn installed_version(wiki: &Path) -> String {
    fs::read_to_string(wiki.join("VERSION")).unwrap_or_default().trim().to_string()
}

/// `2026-07-14c "Mort"` -> `2026-07-14c`
fn version_key(v: &str) -> String {
    v.split_whitespace().next().unwrap_or("").to_string()
}

fn valid_version(v: &str) -> bool {
    let b = v.as_bytes();
    b.len() >= 10
        && b[..10]
            .iter()
            .enumerate()
            .all(|(i, c)| if i == 4 || i == 7 { *c == b'-' } else { c.is_ascii_digit() })
}

/// DokuWiki versions are date-based (YYYY-MM-DD + optional hotfix letter), so string order is release order.
fn is_newer(remote: &str, local: &str) -> bool {
    let (r, l) = (version_key(remote), version_key(local));
    valid_version(&r) && (!valid_version(&l) || r > l)
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn auto_check_enabled(l: &Layout) -> bool {
    !l.support.join("update-auto-off").exists()
}

fn check_due(l: &Layout) -> bool {
    fs::read_to_string(l.support.join("update-last-check"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|t| now_secs().saturating_sub(t) >= CHECK_INTERVAL_SECS)
        .unwrap_or(true)
}

fn curl_text(url: &str) -> Result<String> {
    let o = Command::new("/usr/bin/curl").args(["-fsSL", "--max-time", "10", url]).output()?;
    if !o.status.success() {
        bail!("curl failed ({})", o.status);
    }
    Ok(String::from_utf8_lossy(&o.stdout).trim().to_string())
}

fn curl_file(url: &str, out: &Path) -> Result<()> {
    let st = Command::new("/usr/bin/curl")
        .args(["-fL", "--silent", "--show-error", "--retry", "2", "--max-time", "600", "-o"])
        .arg(out)
        .arg(url)
        .status()?;
    if !st.success() {
        bail!("download failed ({st})");
    }
    Ok(())
}

enum Choice {
    Update,
    Skip,
    Later,
}

/// Result of one update check.
enum Outcome {
    UpToDate(String),
    Unreachable,
    Offered, // a dialog was shown (or the version was skipped); nothing more to report
    Started, // user confirmed; an update is being applied
}

fn ask_update(what: &str, current: &str, new: &str, note: &str) -> Choice {
    let msg = format!("{what} {new} is available (you have {current}).\n\n{note}");
    let script = format!(
        "display dialog \"{}\" with title \"{APP_NAME}\" buttons {{\"Skip This Version\", \"Later\", \"Update\"}} default button \"Update\" cancel button \"Later\" with icon note",
        esc(&msg)
    );
    match Command::new("osascript").args(["-e", &script]).output() {
        Ok(o) if o.status.success() => {
            let out = String::from_utf8_lossy(&o.stdout);
            if out.contains("Update") {
                Choice::Update
            } else if out.contains("Skip") {
                Choice::Skip
            } else {
                Choice::Later
            }
        }
        _ => Choice::Later, // "Later" is the cancel button (non-zero exit)
    }
}

fn is_skipped(l: &Layout, file: &str, version: &str) -> bool {
    fs::read_to_string(l.support.join(file)).map(|s| s.trim() == version).unwrap_or(false)
}

/// Runs on a background thread. `manual` = user chose "Check for Updates…" (always reports a result);
/// automatic checks are silent unless an update is found. One update is applied at a time.
fn check_all(l: &Layout, proxy: &EventLoopProxy<UserEvent>, manual: bool) {
    let _ = fs::write(l.support.join("update-last-check"), now_secs().to_string());
    let a = check_dokuwiki(l, proxy, manual);
    if matches!(a, Outcome::Started) {
        return; // PHP gets checked next time, so the two updates never overlap
    }
    let b = check_php(l, proxy, manual);
    if !manual {
        return;
    }
    match (a, b) {
        (Outcome::UpToDate(x), Outcome::UpToDate(y)) => info(&format!("{x} and {y} are up to date.")),
        (Outcome::Unreachable, Outcome::Unreachable)
        | (Outcome::Unreachable, Outcome::UpToDate(_))
        | (Outcome::UpToDate(_), Outcome::Unreachable) => info("Couldn't check for updates. Are you online?"),
        _ => {}
    }
}

fn check_dokuwiki(l: &Layout, proxy: &EventLoopProxy<UserEvent>, manual: bool) -> Outcome {
    let installed = installed_version(&l.wiki);
    let remote = match curl_text(VERSION_URL) {
        Ok(v) if valid_version(&version_key(&v)) => v,
        _ => return Outcome::Unreachable,
    };
    if !is_newer(&remote, &installed) {
        return Outcome::UpToDate(format!("DokuWiki {}", version_key(&installed)));
    }
    let rk = version_key(&remote);
    if !manual && is_skipped(l, "update-skipped", &rk) {
        return Outcome::Offered;
    }
    let note = "Updating first makes a backup of your wiki. Your pages, media and settings are kept.";
    match ask_update("DokuWiki", &installed, &remote, note) {
        Choice::Later => return Outcome::Offered,
        Choice::Skip => {
            let _ = fs::write(l.support.join("update-skipped"), &rk);
            return Outcome::Offered;
        }
        Choice::Update => {}
    }
    match prepare_update(l, &installed) {
        Ok(staging) => {
            let _ = proxy.send_event(UserEvent::ApplyUpdate { staging, version: remote });
            Outcome::Started
        }
        Err(e) => {
            alert(&format!("The update was not applied. Nothing was changed.\n\n{e:#}"));
            Outcome::Offered
        }
    }
}

/// Newest `php-X.Y.Z-cli-macos-aarch64.tar.gz` in the directory listing for the given major.minor line.
fn latest_php_in_line(listing: &str, major: u32, minor: u32) -> Option<String> {
    let suffix = "-cli-macos-aarch64.tar.gz";
    let mut best: Option<(u32, u32, u32)> = None;
    for (i, _) in listing.match_indices("php-") {
        let rest = &listing[i + 4..];
        let Some(end) = rest.find(suffix) else { continue };
        let seg = &rest[..end];
        if !seg.chars().all(|c| c.is_ascii_digit() || c == '.') {
            continue;
        }
        if let Some(v) = parse_ver(seg) {
            if v.0 == major && v.1 == minor && best.map_or(true, |b| v > b) {
                best = Some(v);
            }
        }
    }
    best.map(|(a, b, c)| format!("{a}.{b}.{c}"))
}

fn check_php(l: &Layout, proxy: &EventLoopProxy<UserEvent>, manual: bool) -> Outcome {
    let Some(installed) = php_version(&l.php) else { return Outcome::Offered };
    let Some((maj, min, _)) = parse_ver(&installed) else { return Outcome::Offered };
    let Ok(listing) = curl_text(&php_list_url()) else { return Outcome::Unreachable };
    let Some(latest) = latest_php_in_line(&listing, maj, min) else { return Outcome::Unreachable };
    if parse_ver(&latest) <= parse_ver(&installed) {
        return Outcome::UpToDate(format!("PHP {installed}"));
    }
    if !manual && is_skipped(l, "php-update-skipped", &latest) {
        return Outcome::Offered;
    }
    let note = "This is a bug-fix/security release of the same PHP line. If the new version doesn't work, ArchiveWiki switches back automatically.";
    match ask_update("PHP", &installed, &latest, note) {
        Choice::Later => return Outcome::Offered,
        Choice::Skip => {
            let _ = fs::write(l.support.join("php-update-skipped"), &latest);
            return Outcome::Offered;
        }
        Choice::Update => {}
    }
    match prepare_php_update(l, &latest) {
        Ok(staged) => {
            let _ = proxy.send_event(UserEvent::ApplyPhp { staged, version: latest });
            Outcome::Started
        }
        Err(e) => {
            alert(&format!("The PHP update was not applied. Nothing was changed.\n\n{e:#}"));
            Outcome::Offered
        }
    }
}

fn parse_ver(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.split('.');
    let a = it.next()?.parse().ok()?;
    let b = it.next()?.parse().ok()?;
    let c = it.next()?.split(|ch: char| !ch.is_ascii_digit()).next()?.parse().ok()?;
    Some((a, b, c))
}

fn php_version(bin: &Path) -> Option<String> {
    let o = Command::new(bin).args(["-n", "-r", "echo PHP_VERSION;"]).output().ok()?;
    if !o.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
    parse_ver(&v).map(|_| v)
}

fn php_modules(bin: &Path) -> Option<Vec<String>> {
    let o = Command::new(bin).args(["-n", "-m"]).output().ok()?;
    if !o.status.success() {
        return None;
    }
    Some(
        String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_lowercase())
            .filter(|l| !l.is_empty() && !l.starts_with('['))
            .collect(),
    )
}

/// Download + unpack + verify a new PHP build. Touches nothing in the live install.
fn prepare_php_update(l: &Layout, new_ver: &str) -> Result<PathBuf> {
    let _ = fs::remove_dir_all(&l.updates);
    fs::create_dir_all(&l.updates)?;
    let tgz = l.updates.join("php.tgz");
    curl_file(&php_download_url(new_ver), &tgz)?;
    let stage = l.updates.join("php-stage");
    fs::create_dir_all(&stage)?;
    let st = Command::new("/usr/bin/tar").arg("-xzf").arg(&tgz).arg("-C").arg(&stage).arg("php").status()?;
    let bin = stage.join("php");
    if !st.success() || !bin.exists() {
        bail!("The download did not contain a php binary.");
    }
    let _ = fs::remove_file(&tgz);
    fs::set_permissions(&bin, fs::Permissions::from_mode(0o755))?;
    let _ = Command::new("/usr/bin/xattr").arg("-c").arg(&bin).status();
    let _ = Command::new("/usr/bin/codesign").args(["--force", "--sign", "-"]).arg(&bin).status();
    let got = php_version(&bin).ok_or_else(|| anyhow!("The new PHP build won't run on this Mac."))?;
    if got != new_ver {
        bail!("Expected PHP {new_ver} but the download reports {got}.");
    }
    let current = php_modules(&l.php).unwrap_or_default();
    let new_mods = php_modules(&bin).ok_or_else(|| anyhow!("Could not read the new PHP's extensions."))?;
    let missing: Vec<&String> = current.iter().filter(|m| !new_mods.contains(m)).collect();
    if !missing.is_empty() {
        bail!(
            "The new build lacks extensions you currently have: {}",
            missing.iter().map(|m| m.as_str()).collect::<Vec<_>>().join(", ")
        );
    }
    Ok(bin)
}

/// Main thread, server stopped: keep the old binary as php.prev, put the new one in place.
fn swap_php(l: &Layout, staged: &Path) -> Result<()> {
    let prev = l.runtime.join("php.prev");
    let _ = fs::remove_file(&prev);
    fs::rename(&l.php, &prev).context("could not set aside the current PHP")?;
    if let Err(e) = fs::rename(staged, &l.php).or_else(|_| fs::copy(staged, &l.php).map(|_| ())) {
        let _ = fs::rename(&prev, &l.php);
        return Err(e.into());
    }
    let _ = fs::remove_dir_all(&l.updates);
    Ok(())
}

fn restore_php(l: &Layout) {
    let prev = l.runtime.join("php.prev");
    if prev.exists() {
        let _ = fs::remove_file(&l.php);
        let _ = fs::rename(&prev, &l.php);
    }
}

fn healthy(port: u16) -> bool {
    Command::new("/usr/bin/curl")
        .args(["-fsS", "-o", "/dev/null", "-m", "15"])
        .arg(format!("http://127.0.0.1:{port}/doku.php"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Start PHP and confirm DokuWiki actually answers. On failure the half-started server is stopped (Drop).
fn start_checked(l: &Layout, port: u16) -> Result<Server> {
    let mut s = Server::start(l, port)?;
    wait_ready(port, &mut s.child)?;
    if !healthy(port) {
        bail!("The wiki did not respond after the restart.");
    }
    Ok(s)
}

/// Download + unpack + verify + back up. Touches nothing in the live wiki except creating the backup zip.
fn prepare_update(l: &Layout, installed: &str) -> Result<PathBuf> {
    let _ = fs::remove_dir_all(&l.updates);
    fs::create_dir_all(&l.updates)?;
    let tgz = l.updates.join("dokuwiki.tgz");
    curl_file(DOWNLOAD_URL, &tgz)?;
    let staging = l.updates.join("staging");
    fs::create_dir_all(&staging)?;
    let st = Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(&tgz)
        .arg("-C")
        .arg(&staging)
        .arg("--strip-components=1")
        .status()?;
    if !st.success() || !staging.join("doku.php").exists() {
        bail!("The download could not be unpacked or doesn't look like DokuWiki.");
    }
    let got = fs::read_to_string(staging.join("VERSION")).unwrap_or_default();
    if !is_newer(&got, installed) {
        bail!(
            "The download is {} but you have {}. The download site may lag behind; try again later.",
            version_key(&got),
            version_key(installed)
        );
    }
    let _ = fs::remove_file(&tgz);
    backup(l, &format!("pre-update-{}", version_key(installed)))?;
    Ok(staging)
}

/// Runs on the main thread with PHP stopped. Snapshots program files first so a failure can be rolled back.
fn apply_update(l: &Layout, staging: &Path) -> Result<()> {
    let _ = fs::remove_dir_all(&l.rollback);
    copy_tree(&l.wiki, &l.rollback, Path::new(""), &["data"])?;
    let res: Result<()> = (|| {
        copy_tree(staging, &l.wiki, Path::new(""), &[])?;
        apply_deleted_list(staging, &l.wiki);
        let _ = fs::remove_file(l.wiki.join("install.php"));
        Ok(())
    })();
    if let Err(e) = res {
        let _ = copy_tree(&l.rollback, &l.wiki, Path::new(""), &[]);
        return Err(e);
    }
    let _ = fs::remove_dir_all(&l.updates);
    Ok(())
}

// ---------------------------------------------------------------- PHP server

struct Server {
    child: Child,
    pidfile: PathBuf,
}

impl Server {
    fn start(l: &Layout, port: u16) -> Result<Self> {
        if fs::metadata(&l.log).map(|m| m.len() > 5_000_000).unwrap_or(false) {
            let _ = fs::remove_file(&l.log);
        }
        let log = fs::OpenOptions::new().create(true).append(true).open(&l.log)?;
        let child = Command::new(&l.php)
            .args([
                "-n",
                "-d", "memory_limit=512M",
                "-d", "upload_max_filesize=128M",
                "-d", "post_max_size=128M",
                "-d", "max_execution_time=120",
                "-d", "display_errors=0",
                "-d", "log_errors=1",
            ])
            .arg("-d")
            .arg(format!("date.timezone={}", detect_tz()))
            .arg("-S")
            .arg(format!("127.0.0.1:{port}"))
            .arg("-t")
            .arg(&l.wiki)
            .arg(&l.router)
            .env("PHP_CLI_SERVER_WORKERS", "4") // DokuWiki fires parallel css/js/fetch requests
            .current_dir(&l.wiki)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log))
            .process_group(0) // own group so workers die with the master
            .spawn()
            .context("failed to launch bundled PHP")?;
        fs::write(&l.pidfile, child.id().to_string())?;
        Ok(Self { child, pidfile: l.pidfile.clone() })
    }

    fn stop(&mut self) {
        if self.child.try_wait().ok().flatten().is_some() {
            return; // already stopped; pidfile may belong to a restarted server
        }
        let pgid = self.child.id() as i32;
        unsafe { libc::kill(-pgid, libc::SIGTERM) };
        for _ in 0..20 {
            if self.child.try_wait().ok().flatten().is_some() {
                break;
            }
            thread::sleep(Duration::from_millis(100));
        }
        if self.child.try_wait().ok().flatten().is_none() {
            unsafe { libc::kill(-pgid, libc::SIGKILL) };
            let _ = self.child.wait();
        }
        let _ = fs::remove_file(&self.pidfile);
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        self.stop();
    }
}

/// If a previous run was force-quit, its PHP may still hold the port.
fn kill_stale(pidfile: &Path) {
    let Ok(s) = fs::read_to_string(pidfile) else { return };
    let Ok(pid) = s.trim().parse::<i32>() else { return };
    if let Ok(out) = Command::new("ps").args(["-p", &pid.to_string(), "-o", "comm="]).output() {
        if String::from_utf8_lossy(&out.stdout).contains("php") {
            unsafe { libc::kill(-pid, libc::SIGTERM) };
            thread::sleep(Duration::from_millis(300));
        }
    }
    let _ = fs::remove_file(pidfile);
}

fn pick_port() -> u16 {
    if TcpListener::bind(("127.0.0.1", PREFERRED_PORT)).is_ok() {
        return PREFERRED_PORT;
    }
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .unwrap_or(PREFERRED_PORT)
}

fn wait_ready(port: u16, child: &mut Child) -> Result<()> {
    let addr: SocketAddr = ([127, 0, 0, 1], port).into();
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(15) {
        if let Some(st) = child.try_wait()? {
            bail!("PHP exited early ({st}). See php-server.log in Application Support.");
        }
        if TcpStream::connect_timeout(&addr, Duration::from_millis(200)).is_ok() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(100));
    }
    bail!("PHP server did not start within 15 seconds.")
}

fn detect_tz() -> String {
    fs::read_link("/etc/localtime")
        .ok()
        .and_then(|p| p.to_string_lossy().split("zoneinfo/").nth(1).map(String::from))
        .unwrap_or_else(|| "UTC".into())
}

// ---------------------------------------------------------------- helpers

fn open_external(url: &str) {
    if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
        let _ = Command::new("open").arg(url).spawn();
    }
}

fn unique_path(p: PathBuf) -> PathBuf {
    if !p.exists() {
        return p;
    }
    let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = p.extension().map(|e| e.to_string_lossy().into_owned());
    for i in 1..1000 {
        let name = match &ext {
            Some(e) => format!("{stem} ({i}).{e}"),
            None => format!("{stem} ({i})"),
        };
        let c = p.with_file_name(name);
        if !c.exists() {
            return c;
        }
    }
    p
}

fn backup(l: &Layout, prefix: &str) -> Result<PathBuf> {
    let dir = dirs::document_dir()
        .ok_or_else(|| anyhow!("Cannot locate ~/Documents"))?
        .join("ArchiveWiki Backups");
    fs::create_dir_all(&dir)?;
    let ts = Command::new("date").arg("+%Y%m%d-%H%M%S").output()?;
    let ts = String::from_utf8_lossy(&ts.stdout).trim().to_string();
    let out = dir.join(format!("{prefix}-{ts}.zip"));
    let st = Command::new("/usr/bin/zip")
        .arg("-qr")
        .arg(&out)
        .args(["data", "conf", "-x", "data/cache/*", "data/tmp/*", "data/locks/*"])
        .current_dir(&l.wiki)
        .status()?;
    if !st.success() {
        bail!("zip exited with {st}");
    }
    Ok(out)
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn dialog(msg: &str, icon: &str) {
    let _ = Command::new("osascript")
        .args([
            "-e",
            &format!(
                "display dialog \"{}\" with title \"{APP_NAME}\" buttons {{\"OK\"}} default button \"OK\" with icon {icon}",
                esc(msg)
            ),
        ])
        .status();
}

fn alert(msg: &str) {
    dialog(msg, "stop");
}

fn info(msg: &str) {
    dialog(msg, "note");
}

fn build_menu(auto_check: bool) -> Result<(Menu, CheckMenuItem)> {
    let acc = |m: Modifiers, c: Code| Some(Accelerator::new(Some(m), c));
    let menu = Menu::new();

    let auto_item = CheckMenuItem::with_id("autocheck", "Check for Updates Automatically", true, auto_check, None);
    let app = Submenu::new(APP_NAME, true);
    app.append_items(&[
        &PredefinedMenuItem::about(None, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("checkupdate", "Check for Updates…", true, None),
        &auto_item,
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::services(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::hide(None),
        &PredefinedMenuItem::hide_others(None),
        &PredefinedMenuItem::show_all(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::quit(None),
    ])?;

    let file = Submenu::new("File", true);
    file.append_items(&[
        &MenuItem::with_id("print", "Print…", true, acc(Modifiers::SUPER, Code::KeyP)),
        &MenuItem::with_id("backup", "Back Up Wiki…", true, acc(Modifiers::SUPER | Modifiers::SHIFT, Code::KeyB)),
        &MenuItem::with_id("folder", "Show Wiki Folder in Finder", true, None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::close_window(None),
    ])?;

    // Without an Edit menu, Cmd+C/V/X/A/Z do nothing inside a WKWebView.
    let edit = Submenu::new("Edit", true);
    edit.append_items(&[
        &PredefinedMenuItem::undo(None),
        &PredefinedMenuItem::redo(None),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::cut(None),
        &PredefinedMenuItem::copy(None),
        &PredefinedMenuItem::paste(None),
        &PredefinedMenuItem::select_all(None),
    ])?;

    let view = Submenu::new("View", true);
    view.append_items(&[
        &MenuItem::with_id("home", "Home", true, acc(Modifiers::SUPER | Modifiers::SHIFT, Code::KeyH)),
        &MenuItem::with_id("back", "Back", true, acc(Modifiers::SUPER, Code::BracketLeft)),
        &MenuItem::with_id("forward", "Forward", true, acc(Modifiers::SUPER, Code::BracketRight)),
        &MenuItem::with_id("reload", "Reload", true, acc(Modifiers::SUPER, Code::KeyR)),
        &PredefinedMenuItem::separator(),
        &PredefinedMenuItem::fullscreen(None),
    ])?;

    let window = Submenu::new("Window", true);
    window.append_items(&[&PredefinedMenuItem::minimize(None), &PredefinedMenuItem::maximize(None)])?;

    menu.append_items(&[&app, &file, &edit, &view, &window])?;
    Ok((menu, auto_item))
}

// ---------------------------------------------------------------- main

fn run() -> Result<()> {
    let layout = Layout::discover()?;
    kill_stale(&layout.pidfile);
    sync_php(&layout.bundle_php, &layout.php)?;
    sync_wiki(&layout.bundle_wiki, &layout.wiki)?;
    fs::write(&layout.router, ROUTER_PHP)?;

    let port = pick_port();
    let mut server = Server::start(&layout, port)?;
    wait_ready(port, &mut server.child)?;

    let origin = format!("http://127.0.0.1:{port}");
    let home = format!("{origin}/doku.php");

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let menu_proxy = event_loop.create_proxy();
    let update_proxy = event_loop.create_proxy();
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = menu_proxy.send_event(UserEvent::Menu(e));
    }));
    let (menu, auto_item) = build_menu(auto_check_enabled(&layout))?;

    let window = WindowBuilder::new()
        .with_title(APP_NAME)
        .with_inner_size(LogicalSize::new(1280.0, 860.0))
        .with_min_inner_size(LogicalSize::new(640.0, 480.0))
        .build(&event_loop)?;

    let nav_origin = origin.clone();
    let webview = WebViewBuilder::new()
        .with_url(&home)
        .with_navigation_handler(move |u: String| {
            if u.starts_with(&nav_origin) || u.starts_with("about:") || u.starts_with("blob:") || u.starts_with("data:") {
                true
            } else {
                open_external(&u);
                false
            }
        })
        .with_new_window_req_handler(|u: String| {
            open_external(&u);
            false
        })
        .with_download_started_handler(|_url: String, dest: &mut PathBuf| {
            if let (Some(dir), Some(name)) = (dirs::download_dir(), dest.file_name().map(|n| n.to_owned())) {
                *dest = unique_path(dir.join(name));
            }
            true
        })
        .with_devtools(cfg!(debug_assertions))
        .build(&window)?;

    event_loop.run(move |event, _, control_flow| {
        let _ = (&window, &menu); // keep alive
        *control_flow = ControlFlow::Wait;
        match event {
            Event::NewEvents(StartCause::Init) => {
                menu.init_for_nsapp();
                if auto_check_enabled(&layout) && check_due(&layout) {
                    let (l, p) = (layout.clone(), update_proxy.clone());
                    thread::spawn(move || {
                        thread::sleep(Duration::from_secs(3)); // let the window appear first
                        check_all(&l, &p, false);
                    });
                }
            }
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                server.stop();
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::ApplyUpdate { staging, version }) => {
                server.stop();
                let res = apply_update(&layout, &staging);
                match Server::start(&layout, port) {
                    Ok(mut s) => {
                        if let Err(e) = wait_ready(port, &mut s.child) {
                            alert(&format!("{e:#}"));
                        }
                        server = s;
                    }
                    Err(e) => {
                        alert(&format!("Could not restart the wiki server: {e:#}"));
                        *control_flow = ControlFlow::Exit;
                        return;
                    }
                }
                let _ = webview.load_url(&home);
                thread::spawn(move || match res {
                    Ok(()) => info(&format!(
                        "Updated to DokuWiki {version}.\n\nA pre-update backup is in Documents/ArchiveWiki Backups."
                    )),
                    Err(e) => alert(&format!("The update failed and your previous version was restored.\n\n{e:#}")),
                });
            }
            Event::UserEvent(UserEvent::ApplyPhp { staged, version }) => {
                server.stop();
                let mut result = swap_php(&layout, &staged);
                let mut new_server = None;
                if result.is_ok() {
                    match start_checked(&layout, port) {
                        Ok(s) => new_server = Some(s),
                        Err(e) => {
                            restore_php(&layout);
                            result = Err(e);
                        }
                    }
                }
                if new_server.is_none() {
                    match start_checked(&layout, port) {
                        Ok(s) => new_server = Some(s),
                        Err(e) => {
                            alert(&format!("Could not restart the wiki server: {e:#}"));
                            *control_flow = ControlFlow::Exit;
                            return;
                        }
                    }
                }
                if let Some(s) = new_server {
                    server = s;
                }
                let _ = webview.load_url(&home);
                thread::spawn(move || match result {
                    Ok(()) => info(&format!("Updated to PHP {version}.")),
                    Err(e) => alert(&format!("The PHP update didn't work, so the previous version was restored.\n\n{e:#}")),
                });
            }
            Event::UserEvent(UserEvent::Menu(ev)) => match ev.id.0.as_str() {
                "home" => { let _ = webview.load_url(&home); }
                "back" => { let _ = webview.evaluate_script("history.back()"); }
                "forward" => { let _ = webview.evaluate_script("history.forward()"); }
                "reload" => { let _ = webview.evaluate_script("location.reload()"); }
                "print" => { let _ = webview.print(); }
                "folder" => { let _ = Command::new("open").arg(&layout.support).spawn(); }
                "backup" => {
                    let l = layout.clone();
                    thread::spawn(move || match backup(&l, "wiki-backup") {
                        Ok(p) => { let _ = Command::new("open").arg("-R").arg(p).spawn(); }
                        Err(e) => alert(&format!("Backup failed: {e:#}")),
                    });
                }
                "checkupdate" => {
                    let (l, p) = (layout.clone(), update_proxy.clone());
                    thread::spawn(move || check_all(&l, &p, true));
                }
                "autocheck" => {
                    let flag = layout.support.join("update-auto-off");
                    if flag.exists() {
                        let _ = fs::remove_file(&flag);
                        auto_item.set_checked(true);
                    } else {
                        let _ = fs::write(&flag, "");
                        auto_item.set_checked(false);
                    }
                }
                _ => {}
            },
            Event::LoopDestroyed => server.stop(),
            _ => {}
        }
    });
}

fn main() {
    if let Err(e) = run() {
        alert(&format!("{e:#}"));
        std::process::exit(1);
    }
}
