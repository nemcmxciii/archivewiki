#[cfg(not(target_os = "macos"))]
compile_error!("ArchiveWiki targets macOS only.");

mod snapshot;
mod starter;

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
use std::sync::atomic::{AtomicBool, Ordering};
use global_hotkey::{
    hotkey::{Code as HkCode, HotKey, Modifiers as HkMods},
    GlobalHotKeyEvent, GlobalHotKeyManager, HotKeyState,
};
use wry::WebViewBuilder;

/// True while the app is locked (window hidden, waiting for the user to authenticate).
static LOCKED: AtomicBool = AtomicBool::new(false);

const APP_NAME: &str = "ArchiveWiki";
/// Fixed preferred port keeps the webview origin (cookies, localStorage) stable across launches.
const PREFERRED_PORT: u16 = 47631;
const ROUTER_PHP: &str = include_str!("router.php");
const OFFLINE_DEFAULTS: &str = include_str!("local.php.seed");

// Wiki branding (replaces DokuWiki's own header logo / favicon). Installed once, never overwrites your own files.
const LOGO_PNG: &[u8] = include_bytes!("../assets/branding/logo.png");
const FAVICON_ICO: &[u8] = include_bytes!("../assets/branding/favicon.ico");
const TOUCH_ICON_PNG: &[u8] = include_bytes!("../assets/branding/apple-touch-icon.png");
// logo.png is wide (disc + "ArchiveWiki" wordmark, 2x for Retina). DokuWiki would draw it at native size, so
// this rule shows it 64px high, and keeps the wiki title text only for screen readers (the wordmark replaces it).
const BRANDING_VERSION: u32 = 3;
const LOGO_DARK_PNG: &[u8] = include_bytes!("../assets/branding/logo_dark.png");
const THEME_CSS: &str = include_str!("theme.css");
const BRANDING_CSS: &str = "\n/* ArchiveWiki branding v2: the logo image includes the wordmark */\n#dokuwiki__header .logo img { height: 64px; width: auto; }\n#dokuwiki__header .logo span { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }\n";

// Starter structure, installed once into the wiki; only files that don't exist yet are written.
const STARTER_VERSION: u32 = 5;
const STARTER: &[(&str, &str)] = &[
    ("data/pages/start.txt", include_str!("starter/start.txt")),
    ("data/pages/sidebar.txt", include_str!("starter/sidebar.txt")),
    ("data/pages/journal/start.txt", include_str!("starter/journal_start.txt")),
    ("data/pages/journal/_template.txt", include_str!("starter/journal_template.txt")),
    ("data/pages/genealogy/start.txt", include_str!("starter/genealogy_start.txt")),
    ("data/pages/genealogy/_template.txt", include_str!("starter/genealogy_template.txt")),
    ("data/pages/genealogy/chart.txt", include_str!("starter/genealogy_chart.txt")),
    ("data/pages/genealogy/chart_blank.txt", include_str!("starter/genealogy_chart_blank.txt")),
    ("data/pages/portfolio/start.txt", include_str!("starter/portfolio_start.txt")),
    ("data/pages/portfolio/residential.txt", include_str!("starter/portfolio_residential.txt")),
    ("data/pages/portfolio/employment.txt", include_str!("starter/portfolio_employment.txt")),
    ("data/pages/portfolio/credentials.txt", include_str!("starter/portfolio_credentials.txt")),
    ("data/pages/reference/start.txt", include_str!("starter/reference_start.txt")),
    ("conf/lang/en/newpage.txt", include_str!("starter/newpage.txt")),
    ("conf/tpl/dokuwiki/style.ini", include_str!("starter/style.ini")),
];

// Starter files shipped by earlier versions. When upgrading, each is removed only if it is still exactly as shipped
// (an edited page is never touched), and namespace folders that end up empty are tidied away.
const LEGACY: &[(&str, &str)] = &[
    ("data/pages/start.txt", include_str!("starter/legacy/start.txt")),
    ("data/pages/sidebar.txt", include_str!("starter/legacy/sidebar.txt")),
    ("data/pages/inbox/start.txt", include_str!("starter/legacy/inbox_start.txt")),
    ("data/pages/inbox/_template.txt", include_str!("starter/legacy/inbox_template.txt")),
    ("data/pages/people/start.txt", include_str!("starter/legacy/people_start.txt")),
    ("data/pages/people/_template.txt", include_str!("starter/legacy/people_template.txt")),
    ("data/pages/records/start.txt", include_str!("starter/legacy/records_start.txt")),
    ("data/pages/records/residential.txt", include_str!("starter/legacy/records_residential.txt")),
    ("data/pages/records/employment.txt", include_str!("starter/legacy/records_employment.txt")),
    ("data/pages/records/credentials.txt", include_str!("starter/legacy/records_credentials.txt")),
    ("data/pages/fieldmanual/start.txt", include_str!("starter/legacy/fieldmanual_start.txt")),
    ("data/pages/genealogy/start.txt", include_str!("starter/legacy/genealogy_start_v4.txt")),
    ("data/pages/genealogy/_template.txt", include_str!("starter/legacy/genealogy_template_v4.txt")),
];

// Automatic backups: at most one per day, the newest N are kept.
const AUTO_BACKUP_INTERVAL_SECS: u64 = 24 * 60 * 60;
const AUTO_BACKUP_KEEP: usize = 10;

// Update source. VERSION on the stable branch is tiny and reliable; the tarball is the official release build.
const VERSION_URL: &str = "https://raw.githubusercontent.com/dokuwiki/dokuwiki/stable/VERSION";
const DOWNLOAD_URL: &str = "https://download.dokuwiki.org/src/dokuwiki/dokuwiki-stable.tgz";
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;
// PHP builds come from this project's own GitHub Releases (made by the "Build PHP" workflow, which includes
// pdo_sqlite). Updates stay within the installed minor line (8.3.x). The repository must be public.
const PHP_REPO: &str = "nemcmxciii/archivewiki";

fn php_list_url() -> String {
    format!("https://github.com/{PHP_REPO}/releases.atom")
}

fn php_download_url(ver: &str) -> String {
    format!("https://github.com/{PHP_REPO}/releases/download/php-{ver}/php-{ver}-cli-macos-aarch64.tar.gz")
}

#[derive(Debug)]
enum UserEvent {
    Menu(MenuEvent),
    /// The user chose a backup zip to restore (already validated and confirmed).
    RestoreBackup { zip: PathBuf },
    /// Lock the app now (menu, inactivity or wake from sleep).
    LockNow,
    /// The user authenticated (or chose to continue without).
    Unlocked,
    /// The user chose to quit from the lock prompt.
    QuitApp,
    /// Result of the test authentication when turning the lock on.
    LockSetup(bool),
    /// Open the editor on a new, timestamped note page in the Journal (menu or global shortcut).
    QuickNote,
    /// Open today's journal page in the editor.
    TodayJournal,
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
    bundle_auth: PathBuf, // pristine auth helper inside the .app
    bundle_plugins: PathBuf, // curated DokuWiki plugins shipped inside the .app
    auth_helper: PathBuf, // locally prepared copy that actually runs
    updates: PathBuf,
    rollback: PathBuf,
    support: PathBuf,
    token: String, // per-launch secret: only the app window can talk to the local wiki server
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
            bundle_auth: res.join("auth-helper"),
            bundle_plugins: res.join("plugins"),
            auth_helper: support.join("runtime").join("auth-helper"),
            updates: support.join("updates"),
            rollback: support.join("wiki-rollback"),
            support,
            token: random_token(),
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
    install_branding(wiki);
    install_starter(wiki);

    // First run: conf/local.php doesn't exist yet, so install.php is kept and ArchiveWiki opens DokuWiki's own
    // setup wizard (wiki name, admin account, ACL policy). Once the wizard has written local.php, DokuWiki asks
    // for install.php to be removed, so we do that and add our offline defaults (once).
    if wiki.join("conf").join("local.php").exists() {
        let _ = fs::remove_file(wiki.join("install.php"));
        apply_offline_defaults(wiki);
    }
    Ok(())
}

/// Put the ArchiveWiki logo/favicon where DokuWiki's template looks for them (data/media/wiki/logo.png,
/// favicon.ico, apple-touch-icon.png). A logo you uploaded yourself is never overwritten; only a logo
/// this app installed earlier (branding v1) is upgraded.
fn install_branding(wiki: &Path) {
    let marker = wiki.join(".archivewiki-branding");
    let have = fs::read_to_string(&marker).ok().and_then(|t| t.trim().parse::<u32>().ok()).unwrap_or(0);
    if have >= BRANDING_VERSION {
        return;
    }
    let dir = wiki.join("data").join("media").join("wiki");
    if fs::create_dir_all(&dir).is_err() {
        return;
    }
    let logo = dir.join("logo.png");
    // v1 installed a square logo (ours, so replace it); with no marker, never overwrite a logo that's already there.
    if have == 1 || (have == 0 && !logo.exists()) {
        let _ = fs::write(&logo, LOGO_PNG);
    }
    for (name, bytes) in [("logo_dark.png", LOGO_DARK_PNG), ("favicon.ico", FAVICON_ICO), ("apple-touch-icon.png", TOUCH_ICON_PNG)] {
        let p = dir.join(name);
        if !p.exists() {
            let _ = fs::write(&p, bytes);
        }
    }
    let css = wiki.join("conf").join("userstyle.css");
    let _ = fs::create_dir_all(wiki.join("conf"));
    let mut text = fs::read_to_string(&css).unwrap_or_default();
    if have < 2 {
        text.push_str(BRANDING_CSS);
    }
    if have < 3 {
        text.push_str(THEME_CSS);
    }
    let _ = fs::write(&css, text);
    let _ = fs::write(&marker, BRANDING_VERSION.to_string());
}

fn apply_offline_defaults(wiki: &Path) {
    let marker = wiki.join(".archivewiki-defaults");
    if marker.exists() {
        return;
    }
    let local = wiki.join("conf").join("local.php");
    let Ok(txt) = fs::read_to_string(&local) else { return };
    let t = txt.trim_end();
    let t = t.strip_suffix("?>").unwrap_or(t).trim_end();
    if fs::write(&local, format!("{t}\n{OFFLINE_DEFAULTS}")).is_ok() {
        let _ = fs::write(&marker, "1");
    }
}

fn random_token() -> String {
    use std::io::Read;
    let mut buf = [0u8; 16];
    if let Ok(mut f) = fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut buf);
    }
    let nanos = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    for (i, b) in buf.iter_mut().enumerate() {
        *b ^= (nanos >> ((i % 4) * 8)) as u8;
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

/// Starter pages, namespace templates and the friendly "page not found" text. Existing files are never touched,
/// so anything you wrote or changed stays as it is; files you delete are not recreated.
fn install_starter(wiki: &Path) {
    let marker = wiki.join(".archivewiki-starter");
    let have = fs::read_to_string(&marker).ok().and_then(|t| t.trim().parse::<u32>().ok()).unwrap_or(0);
    if have >= STARTER_VERSION {
        return;
    }
    starter::retire_legacy(wiki, LEGACY);
    starter::write_missing(wiki, STARTER);
    let _ = fs::write(&marker, STARTER_VERSION.to_string());
}

/// Curated plugins shipped with the app (struct, sqlite, bureaucracy, move, tag, pagelist, include, wrap, gallery).
/// Each is copied into lib/plugins once, and only if you don't already have it. A plugin you remove later is not
/// re-added, and one you installed or updated yourself is never touched.
fn install_plugins(l: &Layout) {
    let Ok(rd) = fs::read_dir(&l.bundle_plugins) else { return };
    let offered_file = l.wiki.join(".archivewiki-plugins");
    let mut offered: Vec<String> = fs::read_to_string(&offered_file)
        .unwrap_or_default()
        .lines()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let target_root = l.wiki.join("lib").join("plugins");
    let mut changed = false;
    for e in rd.flatten() {
        if !e.path().is_dir() {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if offered.contains(&name) {
            continue;
        }
        let dst = target_root.join(&name);
        if !dst.exists() && copy_tree(&e.path(), &dst, Path::new(""), &[]).is_err() {
            let _ = fs::remove_dir_all(&dst);
            continue;
        }
        offered.push(name);
        changed = true;
    }
    if changed {
        let _ = fs::write(&offered_file, offered.join("\n"));
    }
}

/// Setup wizard until DokuWiki has been configured, the wiki itself afterwards.
fn start_url(l: &Layout, origin: &str) -> String {
    // The secret in the first URL makes the server set a session cookie for this window (see router.php).
    if l.wiki.join("conf").join("local.php").exists() {
        format!("{origin}/doku.php?aw={}", l.token)
    } else {
        format!("{origin}/install.php?aw={}", l.token)
    }
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

/// Newest `php-X.Y.Z` release tag in the feed text for the given major.minor line.
fn latest_php_in_line(listing: &str, major: u32, minor: u32) -> Option<String> {
    let mut best: Option<(u32, u32, u32)> = None;
    for (i, _) in listing.match_indices("php-") {
        let raw: String = listing[i + 4..].chars().take_while(|c| c.is_ascii_digit() || *c == '.').collect();
        let seg = raw.trim_end_matches('.');
        if seg.split('.').count() != 3 {
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

fn healthy(port: u16, token: &str) -> bool {
    Command::new("/usr/bin/curl")
        .args(["-fsS", "-o", "/dev/null", "-m", "15", "-H"])
        .arg(format!("Cookie: aw_session={token}"))
        .arg(format!("http://127.0.0.1:{port}/doku.php"))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Start PHP and confirm DokuWiki actually answers. On failure the half-started server is stopped (Drop).
fn start_checked(l: &Layout, port: u16) -> Result<Server> {
    let mut s = Server::start(l, port)?;
    wait_ready(port, &mut s.child)?;
    if !healthy(port, &l.token) {
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
            .env("ARCHIVEWIKI_TOKEN", &l.token)
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

fn backup_dir() -> Result<PathBuf> {
    Ok(dirs::document_dir().ok_or_else(|| anyhow!("Cannot locate ~/Documents"))?.join("ArchiveWiki Backups"))
}

fn backup(l: &Layout, prefix: &str) -> Result<PathBuf> {
    let dir = backup_dir()?;
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

fn auto_backup_enabled(l: &Layout) -> bool {
    !l.support.join("backup-auto-off").exists()
}

fn auto_backup_due(l: &Layout) -> bool {
    fs::read_to_string(l.support.join("last-auto-backup"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|t| now_secs().saturating_sub(t) >= AUTO_BACKUP_INTERVAL_SECS)
        .unwrap_or(true)
}

/// Keep only the newest `keep` automatic backups (manual and pre-update/pre-restore backups are never deleted).
fn prune_auto_backups(keep: usize) {
    let Ok(dir) = backup_dir() else { return };
    let Ok(rd) = fs::read_dir(&dir) else { return };
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("auto-backup-") && n.ends_with(".zip"))
                .unwrap_or(false)
        })
        .collect();
    files.sort(); // timestamps in the names sort oldest-first
    if files.len() > keep {
        for old in &files[..files.len() - keep] {
            let _ = fs::remove_file(old);
        }
    }
}

/// Background loop: one automatic backup per day while the app is open (first check 45 s after launch).
fn auto_backup_loop(l: Layout) {
    thread::sleep(Duration::from_secs(45));
    loop {
        if auto_backup_enabled(&l) && auto_backup_due(&l) && backup(&l, "auto-backup").is_ok() {
            let _ = fs::write(l.support.join("last-auto-backup"), now_secs().to_string());
            prune_auto_backups(AUTO_BACKUP_KEEP);
        }
        thread::sleep(Duration::from_secs(60 * 60));
    }
}

/// Background thread: let the user pick a backup, validate it, confirm, then hand it to the main thread.
fn restore_flow(proxy: &EventLoopProxy<UserEvent>) {
    let start = backup_dir().ok().filter(|d| d.exists()).map(|d| d.to_string_lossy().into_owned());
    let loc = start.map(|d| format!(" default location (POSIX file \"{}\")", esc(&d))).unwrap_or_default();
    let script = format!("POSIX path of (choose file with prompt \"Choose a backup to restore\" of type {{\"zip\"}}{loc})");
    let Ok(out) = Command::new("osascript").args(["-e", &script]).output() else { return };
    if !out.status.success() {
        return; // cancelled
    }
    let zip = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
    if let Err(e) = validate_backup_zip(&zip) {
        alert(&format!("That file can't be restored.\n\n{e:#}"));
        return;
    }
    let name = zip.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let msg = format!(
        "Restore \"{name}\"?\n\nYour current wiki is saved first (a pre-restore backup), then replaced by this backup. Pages created since the backup was made will be removed from the wiki."
    );
    let script = format!(
        "display dialog \"{}\" with title \"{APP_NAME}\" buttons {{\"Cancel\", \"Restore\"}} default button \"Cancel\" cancel button \"Cancel\" with icon caution",
        esc(&msg)
    );
    if let Ok(o) = Command::new("osascript").args(["-e", &script]).output() {
        if o.status.success() && String::from_utf8_lossy(&o.stdout).contains("Restore") {
            let _ = proxy.send_event(UserEvent::RestoreBackup { zip });
        }
    }
}

/// Only zips made by this app (entries under data/ and conf/, no odd paths) are accepted.
fn validate_backup_zip(zip: &Path) -> Result<()> {
    let out = Command::new("/usr/bin/unzip").arg("-Z1").arg(zip).output()?;
    if !out.status.success() {
        bail!("It isn't a readable zip file.");
    }
    let list = String::from_utf8_lossy(&out.stdout);
    let mut has_data = false;
    for name in list.lines() {
        let snapshot_meta = name == snapshot::MANIFEST || name == "SNAPSHOT.txt";
        if name.contains("..") || name.starts_with('/') || !(snapshot_meta || name.starts_with("data/") || name.starts_with("conf/")) {
            bail!("It doesn't look like an ArchiveWiki backup (unexpected entry: {name}).");
        }
        if name.starts_with("data/") {
            has_data = true;
        }
    }
    if !has_data {
        bail!("It doesn't contain any wiki data.");
    }
    Ok(())
}

/// Main thread, server stopped. Saves the current wiki first; on failure puts the previous data back.
fn restore_backup(l: &Layout, zip: &Path) -> Result<()> {
    let safety = backup(l, "pre-restore").context("could not save your current wiki first")?;
    let trash = l.support.join("restore-previous");
    let _ = fs::remove_dir_all(&trash);
    fs::create_dir_all(&trash)?;
    let data = l.wiki.join("data");
    if data.exists() {
        fs::rename(&data, trash.join("data")).context("could not set aside the current data")?;
    }
    // (a snapshot also carries MANIFEST.sha256 and SNAPSHOT.txt; those are not part of the wiki)
    let st = Command::new("/usr/bin/unzip")
        .arg("-oq")
        .arg(zip)
        .arg("-x")
        .arg(snapshot::MANIFEST)
        .arg("SNAPSHOT.txt")
        .arg("-d")
        .arg(&l.wiki)
        .status();
    let ok = matches!(&st, Ok(s) if s.success());
    if !ok {
        let _ = fs::remove_dir_all(&data);
        let _ = fs::rename(trash.join("data"), &data);
        let _ = Command::new("/usr/bin/unzip").arg("-oq").arg(&safety).arg("-x").arg("data/*").arg("-d").arg(&l.wiki).status();
        bail!("The backup could not be unpacked. Your previous wiki was kept.");
    }
    for d in ["cache", "tmp", "locks", "log", "index", "meta", "attic", "media", "media_attic", "media_meta", "pages"] {
        let _ = fs::create_dir_all(data.join(d));
    }
    Ok(())
}

// ---------------------------------------------------------------- app lock

fn appearance_mode(l: &Layout) -> String {
    match fs::read_to_string(l.support.join("appearance")).ok().as_deref().map(str::trim) {
        Some("light") => "light".into(),
        Some("dark") => "dark".into(),
        _ => "system".into(),
    }
}

/// Runs at the start of every page: applies the saved appearance (the wiki's CSS reacts to data-aw-theme).
fn appearance_script(default_mode: &str) -> String {
    format!(
        "(function(){{var t;try{{t=localStorage.getItem('aw-theme');}}catch(e){{}}t=t||'{default_mode}';\
         function a(){{document.documentElement.setAttribute('data-aw-theme',t);}}\
         if(document.documentElement){{a();}}else{{document.addEventListener('DOMContentLoaded',a);}}}})();"
    )
}

fn hotkey_enabled(l: &Layout) -> bool {
    !l.support.join("hotkey-off").exists()
}

fn stamp(fmt: &str) -> String {
    Command::new("date").arg(fmt).output().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string()).unwrap_or_default()
}

fn lock_enabled(l: &Layout) -> bool {
    l.support.join("lock-on").exists()
}

/// Minutes of inactivity before locking; 0 = never (only at launch and via Lock Now).
fn lock_idle_minutes(l: &Layout) -> u32 {
    fs::read_to_string(l.support.join("lock-idle")).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(15)
}

/// Copy the helper out of the .app and prepare it locally (clear quarantine, ad-hoc sign), like PHP.
fn install_auth_helper(l: &Layout) {
    if !l.bundle_auth.exists() {
        return;
    }
    if let Some(dir) = l.auth_helper.parent() {
        let _ = fs::create_dir_all(dir);
    }
    let _ = fs::remove_file(&l.auth_helper);
    if fs::copy(&l.bundle_auth, &l.auth_helper).is_ok() {
        let _ = fs::set_permissions(&l.auth_helper, fs::Permissions::from_mode(0o755));
        prepare_binary(&l.auth_helper);
    }
}

enum AuthResult {
    Confirmed,
    Failed,
    Unavailable,
}

fn authenticate(l: &Layout, reason: &str) -> AuthResult {
    if !l.auth_helper.exists() {
        return AuthResult::Unavailable;
    }
    match Command::new(&l.auth_helper).arg(reason).status() {
        Ok(s) => match s.code() {
            Some(0) => AuthResult::Confirmed,
            Some(2) => AuthResult::Unavailable,
            _ => AuthResult::Failed,
        },
        Err(_) => AuthResult::Unavailable,
    }
}

/// Two- or three-button dialog; returns the button that was pressed.
fn choose(msg: &str, buttons: &[&str]) -> Option<String> {
    let list = buttons.iter().map(|b| format!("\"{b}\"")).collect::<Vec<_>>().join(", ");
    let default = buttons.last().copied().unwrap_or("OK");
    let script = format!(
        "display dialog \"{}\" with title \"{APP_NAME}\" buttons {{{list}}} default button \"{default}\" with icon note",
        esc(msg)
    );
    let out = Command::new("osascript").args(["-e", &script]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&out.stdout);
    buttons.iter().find(|b| text.contains(&format!("button returned:{b}"))).map(|b| b.to_string())
}

/// Background thread while locked: prompt until the user authenticates or quits.
fn unlock_flow(l: &Layout, proxy: &EventLoopProxy<UserEvent>) {
    loop {
        match authenticate(l, "Unlock ArchiveWiki") {
            AuthResult::Confirmed => {
                let _ = proxy.send_event(UserEvent::Unlocked);
                return;
            }
            AuthResult::Unavailable => {
                let msg = "The app lock is on, but this Mac can't confirm your identity right now (no Touch ID or login password available).";
                match choose(msg, &["Quit", "Open Anyway"]).as_deref() {
                    Some("Open Anyway") => {
                        let _ = proxy.send_event(UserEvent::Unlocked);
                    }
                    _ => {
                        let _ = proxy.send_event(UserEvent::QuitApp);
                    }
                }
                return;
            }
            AuthResult::Failed => match choose("ArchiveWiki is locked.", &["Quit", "Try Again"]).as_deref() {
                Some("Try Again") => continue,
                _ => {
                    let _ = proxy.send_event(UserEvent::QuitApp);
                    return;
                }
            },
        }
    }
}

/// Seconds since the last keyboard/mouse input anywhere on the Mac (no permissions needed).
fn hid_idle_secs() -> Option<u64> {
    let out = Command::new("/usr/sbin/ioreg").args(["-c", "IOHIDSystem"]).output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let pos = text.find("\"HIDIdleTime\"")?;
    let digits: String = text[pos..].chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
    digits.parse::<u64>().ok().map(|ns| ns / 1_000_000_000)
}

/// Locks after the chosen idle time, or when the Mac has been asleep (detected as a gap between polls).
fn lock_monitor(l: Layout, proxy: EventLoopProxy<UserEvent>) {
    let mut last = SystemTime::now();
    loop {
        thread::sleep(Duration::from_secs(20));
        let now = SystemTime::now();
        let gap = now.duration_since(last).unwrap_or_default();
        last = now;
        if !lock_enabled(&l) || LOCKED.load(Ordering::SeqCst) {
            continue;
        }
        let minutes = lock_idle_minutes(&l);
        if minutes == 0 {
            continue;
        }
        let slept = gap > Duration::from_secs(90);
        let idle = hid_idle_secs().unwrap_or(0);
        if slept || idle >= u64::from(minutes) * 60 {
            let _ = proxy.send_event(UserEvent::LockNow);
        }
    }
}

// ---------------------------------------------------------------- snapshots

fn snapshot_dir() -> Result<PathBuf> {
    Ok(dirs::document_dir().ok_or_else(|| anyhow!("Cannot locate ~/Documents"))?.join("ArchiveWiki Snapshots"))
}

/// A snapshot is a restorable zip of data/ and conf/ plus MANIFEST.sha256 (a SHA-256 for every file) and
/// SNAPSHOT.txt, with the zip's own checksum saved beside it. Files are cloned first so the manifest and the zip agree.
fn create_snapshot(l: &Layout) -> Result<(PathBuf, usize)> {
    let dir = snapshot_dir()?;
    fs::create_dir_all(&dir)?;
    let stage = l.support.join("snapshot-staging");
    let _ = fs::remove_dir_all(&stage);
    fs::create_dir_all(&stage)?;
    let result = (|| -> Result<(PathBuf, usize)> {
        for top in snapshot::TOP_DIRS {
            let src = l.wiki.join(top);
            if !src.exists() {
                continue;
            }
            // APFS clone: instant and uses no extra space. Falls back to a normal copy.
            let cloned = Command::new("/bin/cp").arg("-Rc").arg(&src).arg(&stage).status().map(|s| s.success()).unwrap_or(false);
            if !cloned {
                let _ = fs::remove_dir_all(stage.join(top));
                let st = Command::new("/bin/cp").arg("-R").arg(&src).arg(&stage).status()?;
                if !st.success() {
                    bail!("could not copy {top}");
                }
            }
        }
        for d in ["cache", "tmp", "locks"] {
            let _ = fs::remove_dir_all(stage.join("data").join(d));
        }
        let count = snapshot::write_manifest(&stage)?;
        let php = php_version(&l.php).unwrap_or_else(|| "unknown".into());
        let about = format!(
            "ArchiveWiki snapshot\nCreated: {}\nApp: ArchiveWiki {}\nDokuWiki: {}\nPHP: {}\nFiles: {}\n\n\
             Contents: the wiki's data/ and conf/ folders (pages, older revisions, media, settings), without caches.\n\
             Verify without ArchiveWiki: unzip this file, then in Terminal run, inside the unzipped folder:\n\
             \x20   shasum -a 256 -c MANIFEST.sha256\n",
            stamp("+%Y-%m-%d %H:%M:%S"),
            env!("CARGO_PKG_VERSION"),
            installed_version(&l.wiki),
            php,
            count
        );
        fs::write(stage.join("SNAPSHOT.txt"), about)?;
        let name = format!("ArchiveWiki-snapshot-{}.zip", stamp("+%Y%m%d-%H%M%S"));
        let out = dir.join(&name);
        let st = Command::new("/usr/bin/zip")
            .arg("-qr")
            .arg(&out)
            .arg(snapshot::MANIFEST)
            .arg("SNAPSHOT.txt")
            .arg("data")
            .arg("conf")
            .current_dir(&stage)
            .status()?;
        if !st.success() {
            bail!("zip exited with {st}");
        }
        let sum = snapshot::sha256_file(&out)?;
        fs::write(dir.join(format!("{name}.sha256")), format!("{sum}  {name}\n"))?;
        Ok((out, count))
    })();
    let _ = fs::remove_dir_all(&stage);
    result
}

/// Returns (everything matched, human-readable report).
fn verify_snapshot_zip(l: &Layout, zip: &Path) -> Result<(bool, String)> {
    let mut notes = String::new();
    let side = PathBuf::from(format!("{}.sha256", zip.display()));
    match fs::read_to_string(&side) {
        Ok(t) => {
            if let Some(want) = t.split_whitespace().next() {
                if snapshot::sha256_file(zip)? != want {
                    return Ok((false, "The zip file doesn't match its saved checksum (the .sha256 file beside it). It has been changed or damaged since it was made.".into()));
                }
                notes.push_str("The zip file matches its saved checksum.\n");
            }
        }
        Err(_) => notes.push_str("No .sha256 file was found beside the zip, so only its contents were checked.\n"),
    }
    let tmp = l.support.join("snapshot-verify");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    let res = (|| -> Result<(bool, String)> {
        let st = Command::new("/usr/bin/unzip").arg("-q").arg(zip).arg("-d").arg(&tmp).status()?;
        if !st.success() {
            bail!("The zip couldn't be unpacked.");
        }
        let v = snapshot::verify_tree(&tmp)?;
        let mut t = notes.clone();
        t.push_str(&format!("{} files checked.\n", v.checked));
        if v.ok() {
            t.push_str("Every file matches its checksum. Nothing was added, removed or changed.");
            return Ok((true, t));
        }
        for (label, list) in [("Changed", &v.changed), ("Missing", &v.missing), ("Unexpected extra", &v.extra)] {
            if !list.is_empty() {
                t.push_str(&format!("\n{label} ({}):\n", list.len()));
                for p in list.iter().take(8) {
                    t.push_str(&format!("  {p}\n"));
                }
                if list.len() > 8 {
                    t.push_str(&format!("  …and {} more\n", list.len() - 8));
                }
            }
        }
        Ok((false, t))
    })();
    let _ = fs::remove_dir_all(&tmp);
    res
}

fn pick_zip(prompt: &str, start: Option<PathBuf>) -> Option<PathBuf> {
    let loc = start
        .filter(|d| d.exists())
        .map(|d| format!(" default location (POSIX file \"{}\")", esc(&d.to_string_lossy())))
        .unwrap_or_default();
    let script = format!("POSIX path of (choose file with prompt \"{}\" of type {{\"zip\"}}{loc})", esc(prompt));
    let out = Command::new("osascript").args(["-e", &script]).output().ok()?;
    if !out.status.success() {
        return None; // cancelled
    }
    Some(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

fn snapshot_flow(l: &Layout) {
    match create_snapshot(l) {
        Ok((p, n)) => {
            let _ = Command::new("open").arg("-R").arg(&p).spawn();
            info(&format!(
                "Snapshot saved ({n} files).\n\nIt contains a checksum for every file, and the zip's own checksum is saved beside it in a .sha256 file. To be able to prove later that nothing changed, keep a copy of that .sha256 file somewhere other than this Mac."
            ));
        }
        Err(e) => alert(&format!("The snapshot could not be created.\n\n{e:#}")),
    }
}

fn verify_flow(l: &Layout) {
    let Some(zip) = pick_zip("Choose a snapshot to verify", snapshot_dir().ok()) else { return };
    match verify_snapshot_zip(l, &zip) {
        Ok((true, t)) => info(&format!("Snapshot verified.\n\n{t}")),
        Ok((false, t)) => alert(&format!("Problem found.\n\n{t}")),
        Err(e) => alert(&format!("The snapshot could not be checked.\n\n{e:#}")),
    }
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

struct MenuState {
    require: CheckMenuItem,
    idle: Vec<(u32, CheckMenuItem)>,
    appearance: Vec<(&'static str, CheckMenuItem)>,
    hotkey: CheckMenuItem,
}

fn build_menu(
    auto_check: bool,
    auto_backup: bool,
    lock_on: bool,
    idle_minutes: u32,
    appearance: &str,
    hotkey_on: bool,
) -> Result<(Menu, CheckMenuItem, CheckMenuItem, MenuState)> {
    let acc = |m: Modifiers, c: Code| Some(Accelerator::new(Some(m), c));
    let menu = Menu::new();

    let auto_item = CheckMenuItem::with_id("autocheck", "Check for Updates Automatically", true, auto_check, None);
    let backup_item = CheckMenuItem::with_id("autobackup", "Back Up Automatically (Daily)", true, auto_backup, None);
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

    let hotkey_item = CheckMenuItem::with_id("hotkey", "Global Shortcut ⌃⌥⌘N for New Note", true, hotkey_on, None);
    let lock_require = CheckMenuItem::with_id("lockrequire", "Require Touch ID or Password", true, lock_on, None);
    let idle_submenu = Submenu::new("Lock After Inactivity", true);
    let mut idle_items: Vec<(u32, CheckMenuItem)> = Vec::new();
    for (mins, label) in [(0u32, "Never (only at launch and Lock Now)"), (5, "5 minutes"), (15, "15 minutes"), (30, "30 minutes")] {
        let it = CheckMenuItem::with_id(format!("idle{mins}"), label, true, mins == idle_minutes, None);
        idle_submenu.append(&it)?;
        idle_items.push((mins, it));
    }

    let file = Submenu::new("File", true);
    file.append_items(&[
        &MenuItem::with_id("newnote", "New Note", true, acc(Modifiers::SUPER | Modifiers::SHIFT, Code::KeyN)),
        &MenuItem::with_id("todayjournal", "Today's Journal Entry", true, acc(Modifiers::SUPER | Modifiers::SHIFT, Code::KeyJ)),
        &hotkey_item,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("print", "Print…", true, acc(Modifiers::SUPER, Code::KeyP)),
        &MenuItem::with_id("backup", "Back Up Wiki…", true, acc(Modifiers::SUPER | Modifiers::SHIFT, Code::KeyB)),
        &backup_item,
        &MenuItem::with_id("restore", "Restore from Backup…", true, None),
        &MenuItem::with_id("snapshot", "Create Archive Snapshot…", true, None),
        &MenuItem::with_id("verifysnap", "Verify Snapshot…", true, None),
        &PredefinedMenuItem::separator(),
        &lock_require,
        &idle_submenu,
        &MenuItem::with_id("locknow", "Lock Now", true, acc(Modifiers::SUPER | Modifiers::SHIFT, Code::KeyL)),
        &PredefinedMenuItem::separator(),
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

    let appearance_menu = Submenu::new("Appearance", true);
    let mut appearance_items: Vec<(&'static str, CheckMenuItem)> = Vec::new();
    for (mode, label) in [("system", "Match macOS"), ("light", "Light"), ("dark", "Dark")] {
        let it = CheckMenuItem::with_id(format!("appearance_{mode}"), label, true, mode == appearance, None);
        appearance_menu.append(&it)?;
        appearance_items.push((mode, it));
    }

    let view = Submenu::new("View", true);
    view.append_items(&[
        &MenuItem::with_id("search", "Search Archive", true, acc(Modifiers::SUPER, Code::KeyK)),
        &MenuItem::with_id("home", "Home", true, acc(Modifiers::SUPER | Modifiers::SHIFT, Code::KeyH)),
        &MenuItem::with_id("back", "Back", true, acc(Modifiers::SUPER, Code::BracketLeft)),
        &MenuItem::with_id("forward", "Forward", true, acc(Modifiers::SUPER, Code::BracketRight)),
        &MenuItem::with_id("reload", "Reload", true, acc(Modifiers::SUPER, Code::KeyR)),
        &PredefinedMenuItem::separator(),
        &appearance_menu,
        &PredefinedMenuItem::fullscreen(None),
    ])?;

    let window = Submenu::new("Window", true);
    window.append_items(&[&PredefinedMenuItem::minimize(None), &PredefinedMenuItem::maximize(None)])?;

    menu.append_items(&[&app, &file, &edit, &view, &window])?;
    Ok((menu, auto_item, backup_item, MenuState { require: lock_require, idle: idle_items, appearance: appearance_items, hotkey: hotkey_item }))
}

// ---------------------------------------------------------------- main

fn run() -> Result<()> {
    let layout = Layout::discover()?;
    kill_stale(&layout.pidfile);
    sync_php(&layout.bundle_php, &layout.php)?;
    install_auth_helper(&layout);
    sync_wiki(&layout.bundle_wiki, &layout.wiki)?;
    install_plugins(&layout);
    fs::write(&layout.router, ROUTER_PHP)?;

    let port = pick_port();
    let mut server = Server::start(&layout, port)?;
    wait_ready(port, &mut server.child)?;

    let origin = format!("http://127.0.0.1:{port}");

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let menu_proxy = event_loop.create_proxy();
    let update_proxy = event_loop.create_proxy();
    let hotkey_proxy = event_loop.create_proxy();
    // Global shortcut (works while another app is in front). Quietly skipped if macOS or another app already owns it.
    let hotkey = HotKey::new(Some(HkMods::CONTROL | HkMods::ALT | HkMods::SUPER), HkCode::KeyN);
    let hotkey_manager = GlobalHotKeyManager::new().ok();
    if hotkey_enabled(&layout) {
        if let Some(m) = &hotkey_manager {
            let _ = m.register(hotkey);
        }
    }
    GlobalHotKeyEvent::set_event_handler(Some(move |e: GlobalHotKeyEvent| {
        if e.state == HotKeyState::Pressed {
            let _ = hotkey_proxy.send_event(UserEvent::QuickNote);
        }
    }));
    MenuEvent::set_event_handler(Some(move |e| {
        let _ = menu_proxy.send_event(UserEvent::Menu(e));
    }));
    let lock_on = lock_enabled(&layout);
    if lock_on {
        LOCKED.store(true, Ordering::SeqCst); // start locked: the window stays hidden until you authenticate
    }
    let (menu, auto_item, backup_item, lock_items) = build_menu(
        auto_check_enabled(&layout),
        auto_backup_enabled(&layout),
        lock_on,
        lock_idle_minutes(&layout),
        &appearance_mode(&layout),
        hotkey_enabled(&layout),
    )?;

    let window = WindowBuilder::new()
        .with_title(APP_NAME)
        .with_visible(!lock_on)
        .with_inner_size(LogicalSize::new(1280.0, 860.0))
        .with_min_inner_size(LogicalSize::new(640.0, 480.0))
        .build(&event_loop)?;

    let nav_origin = origin.clone();
    let webview = WebViewBuilder::new()
        .with_url(&start_url(&layout, &origin))
        .with_initialization_script(&appearance_script(&appearance_mode(&layout)))
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
                {
                    let l = layout.clone();
                    thread::spawn(move || auto_backup_loop(l));
                }
                {
                    let (l, p) = (layout.clone(), update_proxy.clone());
                    thread::spawn(move || lock_monitor(l, p));
                }
                if LOCKED.load(Ordering::SeqCst) {
                    let (l, p) = (layout.clone(), update_proxy.clone());
                    thread::spawn(move || unlock_flow(&l, &p));
                }
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
                let _ = webview.load_url(&start_url(&layout, &origin));
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
                let _ = webview.load_url(&start_url(&layout, &origin));
                thread::spawn(move || match result {
                    Ok(()) => info(&format!("Updated to PHP {version}.")),
                    Err(e) => alert(&format!("The PHP update didn't work, so the previous version was restored.\n\n{e:#}")),
                });
            }
            Event::UserEvent(UserEvent::RestoreBackup { zip }) => {
                server.stop();
                let res = restore_backup(&layout, &zip);
                match start_checked(&layout, port) {
                    Ok(s) => server = s,
                    Err(e) => {
                        alert(&format!("Could not restart the wiki server: {e:#}"));
                        *control_flow = ControlFlow::Exit;
                        return;
                    }
                }
                let _ = webview.load_url(&start_url(&layout, &origin));
                thread::spawn(move || match res {
                    Ok(()) => info("Backup restored. Your previous wiki was saved as a pre-restore backup in Documents/ArchiveWiki Backups."),
                    Err(e) => alert(&format!("The restore didn't complete.\n\n{e:#}")),
                });
            }
            Event::UserEvent(UserEvent::QuickNote) => {
                if !LOCKED.load(Ordering::SeqCst) {
                    window.set_visible(true);
                    window.set_focus();
                    let _ = webview.load_url(&format!("{origin}/doku.php?id=journal:note_{}&do=edit", stamp("+%Y-%m-%d_%H%M%S")));
                }
            }
            Event::UserEvent(UserEvent::TodayJournal) => {
                if !LOCKED.load(Ordering::SeqCst) {
                    window.set_visible(true);
                    window.set_focus();
                    let _ = webview.load_url(&format!("{origin}/doku.php?id=journal:{}&do=edit", stamp("+%Y-%m-%d")));
                }
            }
            Event::UserEvent(UserEvent::LockNow) => {
                if lock_enabled(&layout) && !LOCKED.swap(true, Ordering::SeqCst) {
                    window.set_visible(false);
                    let (l, p) = (layout.clone(), update_proxy.clone());
                    thread::spawn(move || unlock_flow(&l, &p));
                }
            }
            Event::UserEvent(UserEvent::Unlocked) => {
                LOCKED.store(false, Ordering::SeqCst);
                window.set_visible(true);
                window.set_focus();
            }
            Event::UserEvent(UserEvent::QuitApp) => {
                server.stop();
                *control_flow = ControlFlow::Exit;
            }
            Event::UserEvent(UserEvent::LockSetup(ok)) => {
                if ok {
                    let _ = fs::write(layout.support.join("lock-on"), "");
                    lock_items.require.set_checked(true);
                } else {
                    lock_items.require.set_checked(false);
                    thread::spawn(|| {
                        alert("The app lock was not turned on, because your identity couldn't be confirmed. Make sure your Mac has a login password (and Touch ID, if you use it), then try again.")
                    });
                }
            }
            Event::UserEvent(UserEvent::Menu(ev)) => match ev.id.0.as_str() {
                "home" => { let _ = webview.load_url(&start_url(&layout, &origin)); }
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
                "search" => {
                    // Focus the wiki's search box; on pages without one (e.g. the setup wizard) open the search page.
                    let _ = webview.evaluate_script(
                        "(function(){var e=document.getElementById('qsearch__in');if(e){e.focus();e.select();}else if(location.pathname.indexOf('install.php')<0){location.href='/doku.php?do=search';}})();",
                    );
                }
                "newnote" => {
                    let _ = update_proxy.send_event(UserEvent::QuickNote);
                }
                "todayjournal" => {
                    let _ = update_proxy.send_event(UserEvent::TodayJournal);
                }
                "hotkey" => {
                    let flag = layout.support.join("hotkey-off");
                    if flag.exists() {
                        let _ = fs::remove_file(&flag);
                        if let Some(m) = &hotkey_manager {
                            let _ = m.register(hotkey);
                        }
                        lock_items.hotkey.set_checked(true);
                    } else {
                        let _ = fs::write(&flag, "");
                        if let Some(m) = &hotkey_manager {
                            let _ = m.unregister(hotkey);
                        }
                        lock_items.hotkey.set_checked(false);
                    }
                }
                "lockrequire" => {
                    let flag = layout.support.join("lock-on");
                    if flag.exists() {
                        let _ = fs::remove_file(&flag);
                        lock_items.require.set_checked(false);
                    } else {
                        // Only turn it on after a successful test, so you can't lock yourself out.
                        lock_items.require.set_checked(false);
                        let (l, p) = (layout.clone(), update_proxy.clone());
                        thread::spawn(move || {
                            let ok = matches!(authenticate(&l, "Turn on the ArchiveWiki app lock"), AuthResult::Confirmed);
                            let _ = p.send_event(UserEvent::LockSetup(ok));
                        });
                    }
                }
                "locknow" => {
                    if lock_enabled(&layout) {
                        let _ = update_proxy.send_event(UserEvent::LockNow);
                    } else {
                        thread::spawn(|| info("Turn on \"Require Touch ID or Password\" in the File menu first."));
                    }
                }
                id if id.starts_with("appearance_") => {
                    let mode = &id["appearance_".len()..];
                    if matches!(mode, "system" | "light" | "dark") {
                        let _ = fs::write(layout.support.join("appearance"), mode);
                        for (m, item) in &lock_items.appearance {
                            item.set_checked(*m == mode);
                        }
                        let _ = webview.evaluate_script(&format!(
                            "try{{localStorage.setItem('aw-theme','{mode}');}}catch(e){{}}document.documentElement.setAttribute('data-aw-theme','{mode}');"
                        ));
                    }
                }
                id if id.starts_with("idle") => {
                    if let Ok(mins) = id[4..].parse::<u32>() {
                        let _ = fs::write(layout.support.join("lock-idle"), mins.to_string());
                        for (m, item) in &lock_items.idle {
                            item.set_checked(*m == mins);
                        }
                    }
                }
                "snapshot" => {
                    let l = layout.clone();
                    thread::spawn(move || snapshot_flow(&l));
                }
                "verifysnap" => {
                    let l = layout.clone();
                    thread::spawn(move || verify_flow(&l));
                }
                "restore" => {
                    let p = update_proxy.clone();
                    thread::spawn(move || restore_flow(&p));
                }
                "autobackup" => {
                    let flag = layout.support.join("backup-auto-off");
                    if flag.exists() {
                        let _ = fs::remove_file(&flag);
                        backup_item.set_checked(true);
                    } else {
                        let _ = fs::write(&flag, "");
                        backup_item.set_checked(false);
                    }
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
