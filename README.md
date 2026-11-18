# ArchiveWiki (Rust, macOS)

A native shell around DokuWiki: a small Rust binary (tao + wry/WKWebView) that launches a
**bundled static PHP** on a loopback port, serves DokuWiki from it, and shows it in a native window.
No Electron, no Node, no system PHP, no network needed at runtime.

## Easiest: double-click
Double-click **Build ArchiveWiki.command**. It checks for Apple's command line tools and Rust (offering to install Rust),
builds `dist/ArchiveWiki.app` + `dist/ArchiveWiki.dmg`, and offers to copy the app to /Applications.
(If macOS blocks the .command because it was downloaded, right-click it → Open.)

## Ready-made .dmg without building locally
Push this folder to a GitHub repo; `.github/workflows/build.yml` builds the .dmg on an Apple-silicon runner
(Actions → Build ArchiveWiki → Run workflow → download the artifact). Apps built this way are downloaded, so macOS
will quarantine them: right-click → Open the first time, or run `xattr -dr com.apple.quarantine /Applications/ArchiveWiki.app`.
Removing that prompt entirely needs an Apple Developer ID certificate and notarization (a paid Apple developer account).

## Build manually (on the Mac)
```bash
xcode-select --install                                   # once
curl https://sh.rustup.rs -sSf | sh                      # once, if Rust isn't installed
./scripts/bundle.sh
open "dist/ArchiveWiki.app"
```
Optional: put a 1024×1024 `assets/icon.png` in place before bundling for an app icon.
DokuWiki comes from `vendor/dokuwiki.tar` (release "Mort" 2026-07-14c, requires PHP 8.2+). To use a different
release, replace that tar (top folder `dokuwiki/`) and delete `resources/dokuwiki`; with no tar present the script downloads stable.
Dev run: `./scripts/fetch-deps.sh && cargo run`.

## How it works
- `Contents/Resources/{php,dokuwiki}` ship inside the .app (pristine, read-only).
- First launch copies DokuWiki to `~/Library/Application Support/ArchiveWiki/wiki/` and opens
  DokuWiki's own **setup wizard** (wiki name, admin account, ACL policy). After the wizard runs, ArchiveWiki removes `install.php` and adds offline defaults (no DokuWiki update check, plain `doku.php?id=` URLs) once. **Your pages/media live in `wiki/data/`.**
- Upgrades (new bundled DokuWiki version) overwrite program files but never existing files in `data/`.
- PHP runs as `php -S 127.0.0.1:47631` (falls back to a free port) with 4 workers, behind `router.php`, which blocks
  `data/ conf/ bin/ inc/ vendor/`, dotfiles, and non-loopback Host headers.
- External links open in your default browser; downloads go to `~/Downloads`.
- Menus: Edit (copy/paste), View (Home, Back/Forward, Reload), File (Print, **Back Up Wiki…** → zip in
  `~/Documents/ArchiveWiki Backups/`, Show Wiki Folder).
- Logs: `php-server.log` in the Application Support folder.

## Backups
- **Automatic:** once a day while the app is open (first check shortly after launch), ArchiveWiki saves `data/` and `conf/` to
  `~/Documents/ArchiveWiki Backups/auto-backup-<time>.zip` and keeps the newest 10. Toggle: File → *Back Up Automatically (Daily)*.
  Manual backups (*Back Up Wiki…*) and the safety backups made before updates/restores are never deleted automatically.
- **Restore:** File → *Restore from Backup…* lets you pick a zip made by this app. It checks the file, asks you to confirm, saves
  your current wiki as a `pre-restore` backup, swaps the data in, and restarts the server. If unpacking fails, the previous data is put back.
  (The data that was replaced is also kept in `Application Support/ArchiveWiki/restore-previous/` until the next restore.)
- Backups are plain zips on your own disk; turn on FileVault if you want them encrypted at rest.

## App lock (optional)
File → **Require Touch ID or Password** turns it on (it tests the prompt first, so you can't lock yourself out). When on:
- the window stays hidden at launch until macOS confirms you (Touch ID, Apple Watch, or your login password);
- **Lock Now** (⇧⌘L) hides the window and asks again to come back;
- **Lock After Inactivity** (5 / 15 / 30 minutes, default 15, or Never) locks after that long without keyboard or mouse input
  anywhere on the Mac, and also after the Mac has been asleep. "Never" means the lock applies only at launch and Lock Now.
- Cancelling the prompt offers *Try Again* or *Quit*. If the Mac can't authenticate at all (no login password set), you're told and can
  choose *Open Anyway*.

What it does **not** do: it doesn't encrypt anything. The wiki's files stay readable to anyone who can read your user account's files
(turn on FileVault for that), and backups are plain zips. It keeps people out of the *window*, e.g. when you step away.
The prompt comes from a small helper (`helper/auth.swift`) built into the app by `scripts/bundle.sh`.

## Private local address
The wiki server only listens on 127.0.0.1, and each launch generates a random secret. The app window presents it once, receives
a session cookie, and every other request without that cookie (another app, a browser tab, a web page) gets "Forbidden".
This is a guard against other software on your Mac reaching the wiki, not against someone who has full access to your account.

## Starter structure
On first run (or the first launch after updating to this version) ArchiveWiki adds, only where nothing exists yet: a start page and
sidebar; Inbox, Journal, People, Records (residential / employment / credentials tables), Field Manual and Reference sections;
page templates for People and Journal (new pages in those sections start from them); and a friendlier "page not found" message.
Edit or delete any of it freely: nothing is recreated or overwritten afterwards.

## Branding
The ArchiveWiki logo (disc + wordmark) replaces DokuWiki's header logo, and the disc replaces the browser-tab icon (`data/media/wiki/logo.png`, `favicon.ico`,
`apple-touch-icon.png`, plus a small rule in `conf/userstyle.css`). It is installed once and never overwrites files you
already have there. The wordmark is part of the logo image, so the wiki title text is hidden visually (kept for screen readers); delete the `ArchiveWiki branding v2` rule in `conf/userstyle.css` to show the title text again. To use a different logo, upload your own `wiki:logo.png` (or `.svg`) in DokuWiki's media manager.
The app icon comes from `assets/icon.png`; the wiki branding files are in `assets/branding/`.

## Updates
- On launch (at most once per 24 h) ArchiveWiki checks DokuWiki's `stable` VERSION and the newest PHP patch release over HTTPS. Offline or failing checks are silent.
- If a newer release exists you get a dialog: **Update / Later / Skip This Version**. Nothing happens without your click.
- On Update: it downloads the official release, unpacks and verifies it, writes `~/Documents/ArchiveWiki Backups/pre-update-<version>-<time>.zip`
  (data + conf), stops PHP, applies the files (never overwriting existing `data/` files, removing files listed in DokuWiki's
  `deleted.files`), restarts and reloads. If applying fails, the previous program files are restored automatically.
- **PHP** works the same way (dialog → confirm), but stays within your installed minor line (e.g. 8.3.x → 8.3.y), so DokuWiki
  compatibility isn't put at risk. A running copy lives in `Application Support/ArchiveWiki/runtime/php`; the signed .app is never modified.
  The new build must run, report the expected version and include every extension the current build has. After swapping it in, ArchiveWiki
  restarts the server and confirms DokuWiki answers; if not, it switches back to the previous PHP (`php.prev`) automatically.
  Moving to a new PHP *line* (e.g. 8.3 → 8.4) is deliberately manual: rebuild with `PHP_MINOR=8.4 ./scripts/bundle.sh`.
- Only one update is applied at a time: if both are available, DokuWiki goes first and PHP is offered on the next check.
- **ArchiveWiki → Check for Updates…** runs a check on demand; **Check for Updates Automatically** turns the launch check off/on.
- Updates live in Application Support, so they persist; a rebuilt app with an older bundled DokuWiki or PHP will not downgrade them.
- Installed plugins/templates are kept, but after a big release check them in Admin → Extension Manager for compatibility.
- Downloads are protected by HTTPS only (neither download site is known to publish checksums for these builds), plus the sanity checks above.

## Notes
- Plugins/templates can be installed from the DokuWiki admin UI or by dropping them in `wiki/lib/plugins/`.
- To enable logins/ACLs later: set `$conf['useacl']=1`, copy `conf/users.auth.php.dist` and `conf/acl.auth.php.dist`
  to the non-`.dist` names, and add a user.
- PHP: a static build made by the **Build PHP** workflow (static-php-cli, 8.3 line, includes `pdo_sqlite` for the sqlite/struct plugins).
  Run Actions → "Build PHP" once before the first "Build ArchiveWiki"; the build script downloads it from this repo's Releases
  (override with `PHP_URL`, `PHP_MINOR`, or `ARCHIVEWIKI_REPO`). The repo must be public (both read its public release feed).
- The app is ad-hoc signed, which is fine for a locally built app. If you copy it to another Mac, right-click → Open.
