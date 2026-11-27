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
Dev run: `./scripts/fetch-deps.sh && cargo run --locked`.

## Dependency integrity

`dependencies.lock` pins the build-time DokuWiki and PHP versions and their GitHub
Release SHA-256 digests. The vendored DokuWiki tar has its own checksum in
`dependencies.sha256`. Build and runtime update paths verify archives before
extraction. To update the bundled versions, build/publish the desired PHP release,
update the version and matching GitHub asset digest in `dependencies.lock`, refresh
the vendored DokuWiki tar and `dependencies.sha256` when changing DokuWiki, then run
the validation workflow before merging. `PHP_VERSION`, `PHP_URL`, `PHP_SHA256`,
`DOKUWIKI_VERSION`, and `DOKUWIKI_SHA256` support explicit build-time overrides;
`ARCHIVEWIKI_PHP_SHA256` and `ARCHIVEWIKI_DOKUWIKI_SHA256` support runtime updater
overrides.

## How it works
- `Contents/Resources/{php,dokuwiki}` ship inside the .app (pristine, read-only).
- At launch, choose **Continue with Last Wiki**, **Log In to an Existing Wiki**, **Create a New Wiki**, or **Import a Backup as a New Wiki**. Existing wikis here means local ArchiveWiki profiles on this Mac; connecting to a hosted/remote DokuWiki is not supported. The login option opens DokuWiki's login page for the selected profile. New wikis open DokuWiki's setup wizard to create an administrator login and choose who can view or edit pages. Setup is local; after it, Home offers simple starting steps. ArchiveWiki then removes `install.php` and adds offline defaults (no DokuWiki update check, plain `doku.php?id=` URLs) once. **Your pages/media live in `wiki/data/`.**
- **File → Manage Wikis** lets you create a separate wiki, import an ArchiveWiki backup as a new wiki, or switch between wikis. Only one wiki is open at a time; switching reopens the app, and each wiki has separate DokuWiki login sessions and recent pages. The existing wiki stays at `Application Support/ArchiveWiki/wiki/`; additional wikis live in `Application Support/ArchiveWiki/wikis/`.
- Upgrades (new bundled DokuWiki version) overwrite program files but never existing files in `data/`.
- PHP runs as `php -S 127.0.0.1:47631` (falls back to a free port) with 4 workers, behind `router.php`, which blocks
  `data/ conf/ bin/ inc/ vendor/`, dotfiles, and non-loopback Host headers.
- Same-origin pop-up windows (including DokuWiki's media manager) open inside ArchiveWiki; external links open in your default browser. Downloads go to `~/Downloads`.
- Home offers a title-based page creation flow and lists your 8 most recently visited pages; recent pages are stored on
  this Mac, kept separate for each wiki, and can be cleared there. If a title already exists, it opens without editing.
- An always-visible in-app toolbar provides **Back**, **Forward** and **Refresh**. Menus: Edit (copy/paste), View (Home,
  Back/Forward, Refresh), File (Manage Wikis, Print, **Back Up & Restore**, Show Wiki Folder).
- Logs: `php-server.log` in the Application Support folder.

## Genealogy
The **ArchiveWiki Genealogy** plugin (`vendor/plugins/awgenealogy`, copied into your wiki like the other bundled plugins) adds:
- a **quick-add form** on the Genealogy start page: create a person with a name and optionally add dates and places; a searchable local directory finds people by name, dates or places;
- **Family Group sheets**: a form for two parents or partners, marriage or partnership details, children, sources, and research notes. Names create or connect individual pages, and saved child relationships feed into the pedigree tree. Family sheets have their own searchable index;
- a **person block** (`<person> ... </person>`) on each person's page: name, sex, born / birthplace, died / deathplace, `father`, `mother`
  (page names, resolved like links in the same section) and `living`. It's shown as a small card on the page;
- a **guided ancestor form**: `{{pedigree>genealogy:jane_doe}}` (add `?4` for a different number of generations). It shows one generation at a time, keeps progress across steps, and lets you save partial work. It lists the
  person, parents, grandparents, great-grandparents and 2nd great-grandparents one generation after another, so it flows vertically
  and works in a narrow window. Every person has name, born, birthplace, died, place of death and a **Living** checkbox.
  Type a name to unlock the next generation, then press **Save changes**. One save validates everything first and applies nothing if
  something is wrong, then creates a page for each new person (with history), links them as father or mother, and updates anyone you edited.
New person pages include the form automatically; `genealogy:chart` has a person picker, and the Genealogy start page has a searchable people directory.
`genealogy:families:start` is the Family Groups workspace.
- a **Tree** view (the link above the form, or `?tree`): the traditional landscape chart with the standard numbering (1 = root, 2 = father,
  3 = mother, 2n / 2n+1 = parents of n), still editable with **+ add** and ✎. `?redact` always shows the tree with living people as "Living".
  `genealogy:chart_blank` is a blank tree to print and fill in by hand.
- printing (⌘P): the form prints as three columns of compact cards; the tree prints on one landscape page.
Who may save follows DokuWiki's permissions. Person details stay in the page's `<person>` block, so no second structured-data store is needed.
**Living:** a person with a death date, or born more than 110 years ago, is saved as
not living. Otherwise the checkbox decides, and it starts ticked for the person and their parents and unticked for older generations,
so you will not share someone who may still be alive by mistake. Marriages, siblings and multiple spouses aren't modelled yet; the chart
follows only the father and mother links.

## Bundled plugins
These DokuWiki plugins ship inside the app and are copied into your wiki once, on first launch (or the first launch after
updating to a version that adds them), only if you don't already have them: **struct** + **sqlite** (structured data), **bureaucracy**
(forms), **move** (rename/move pages, updating links), **tag** + **pagelist**, **include**, **wrap** and **gallery**.
Removing one is respected (it isn't re-added), and a plugin you installed or updated yourself is never overwritten.
Later plugin updates come through DokuWiki's Extension Manager (needs internet) or a new ArchiveWiki release.
Not bundled on purpose: *dw2pdf* (adds ~45 MB; use File → Print → Save as PDF instead) and *graphviz* (needs the separate `dot` program).
Sources and licenses: `THIRD_PARTY_NOTICES.md`. The plugin versions are untested against this exact DokuWiki release, so check
Admin → Extension Manager after the first launch.

## Backups
- For everyday use, open **File → Back Up & Restore**:
  - **Back Up Now** saves a copy of the active wiki's pages, media, and settings as a zip in `~/Documents/ArchiveWiki Backups/`. Additional wikis use a named subfolder there; the existing default wiki keeps using the original folder.
  - **Back Up Automatically (Daily)** is on by default for each wiki. It runs while the app is open and keeps the newest 10 automatic backups per wiki.
  - **Restore from Backup…** lets you choose a backup zip. It checks the file, rejects unsafe paths and
  symbolic links, asks you to confirm, saves your current wiki as a `pre-restore` backup, stages the archive, then replaces `data/`
  and `conf/` together before restarting the server. If staging or replacement fails, the previous folders are put back.
  (The data and configuration that were replaced are kept with that wiki's profile until the next restore.)
- Older automatic backups are removed; manual and safety backups made before updates/restores are never deleted automatically.
- **Snapshots & Verification (Advanced)** is for checking file integrity with checksums; it is optional for everyday backups. Snapshots can also be restored.
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

## Snapshots (integrity checking)
**File → Back Up & Restore → Snapshots & Verification (Advanced) → Create Archive Snapshot…** saves `~/Documents/ArchiveWiki Snapshots/ArchiveWiki-snapshot-<time>.zip` for the default wiki, or a named subfolder for another wiki: its `data/` and `conf/`
(pages, older revisions, media, settings; no caches), plus `MANIFEST.sha256` (a SHA-256 for every file) and `SNAPSHOT.txt`
(when, app/DokuWiki/PHP versions, file count). The zip's own checksum is saved beside it as `<name>.zip.sha256`.
**Verify Snapshot…** in the same submenu safely stages a snapshot and recomputes every hash, reporting changed, missing or unexpected files.
You can also verify with no ArchiveWiki at all: unzip, then run `shasum -a 256 -c MANIFEST.sha256` inside the folder.
A snapshot is also a valid backup: *Restore from Backup…* accepts it.

What this proves, and what it doesn't: it detects accidental damage and any later change to the files, *if you trust the checksums*.
The manifest travels inside the zip, so someone who could rewrite both the files and the manifest could fake a clean result. For real
tamper-evidence, keep a copy of the zip's `.sha256` file (or its one-line hash) somewhere separate, such as a printout, an email to yourself,
or another drive. Snapshots are plain, unencrypted zips.

## Private local address
The wiki server only listens on 127.0.0.1, and each launch generates a random secret. The app window presents it once, receives
a session cookie, and every other request without that cookie (another app, a browser tab, a web page) gets "Forbidden".
This is a guard against other software on your Mac reaching the wiki, not against someone who has full access to your account.

## Starter structure
On first run (or the first launch after updating to this version) ArchiveWiki adds, only where nothing exists yet: a start page and
sidebar; Journal, Genealogy, Portfolio (residential / employment / credentials tables) and Reference sections;
page templates for Genealogy and Journal (new pages in those sections start from them); and a friendlier "page not found" message.
Edit or delete any of it freely: nothing is recreated or overwritten afterwards.
When upgrading from an earlier layout (Inbox, People, Records, Field Manual), pages from the old layout are removed only if you never
edited them; anything you changed stays where it is.

The local WEB Bible includes the standard 66 books plus 15 Deuterocanonical books or sections from the public-domain
WEB Updated 2024 edition. The Greek Esther and Daniel are provided as clearly labeled complete editions alongside the
standard WEB versions. These chapter pages are added only when missing; existing pages and personal study notes are
left untouched.

## Search
**View → Search Archive** (⌘K) jumps to the wiki's search box (or to the search page if the current page has none). Search stays inside the app:
ArchiveWiki deliberately does *not* add your pages to macOS Spotlight, because Spotlight would show page titles and text outside the app
window, even while the app lock is on.

## Quick capture
- **File → New Note** (⇧⌘N) opens DokuWiki's editor on a new, timestamped page in the Journal (`journal:note_<date>_<time>`); rename or move it later.
- **File → Today's Journal Entry** (⇧⌘J) opens today's page in the Journal (`journal:<date>`) in the editor, from the journal template.
- **Global shortcut ⌃⌥⌘N** does the same as New Note even while another app is in front (toggle in the File menu). If macOS or another
  app already uses that combination, it's silently skipped. While the app is locked, the shortcut does nothing.
- Opening a note navigates the window, so any unsaved edit on the current page is left to DokuWiki's draft autosave.

## Theme and dark mode
- **Light theme:** ArchiveWiki's slate-blue palette is applied through DokuWiki's own template settings (`conf/tpl/dokuwiki/style.ini`, written once
  if you don't have one; edit it, or use Admin → Template Style Settings), with the macOS system font.
- **Dark mode:** follows macOS by default. **View → Appearance** lets you force *Light* or *Dark*. It's done in CSS (the page is
  colour-inverted, images are left alone, and the header uses `logo_dark.png`), so it covers every page and plugin, but a few
  colours can look slightly different from a hand-tuned dark theme. Printing always uses the light theme.
- If you replace the header logo with your own, delete the `html ... #dokuwiki__header .logo img { content: url(...) }` lines in
  `conf/userstyle.css` so the dark-mode swap doesn't put ArchiveWiki's logo back.

## Branding
The ArchiveWiki logo (disc + wordmark) replaces DokuWiki's header logo, and the disc replaces the browser-tab icon (`data/media/wiki/logo.png`, `favicon.ico`,
`apple-touch-icon.png`, plus a small rule in `conf/userstyle.css`). It is installed once and never overwrites files you
already have there. The wordmark is part of the logo image, so the wiki title text is hidden visually (kept for screen readers); delete the `ArchiveWiki branding v2` rule in `conf/userstyle.css` to show the title text again. To use a different logo, upload your own `wiki:logo.png` (or `.svg`) in DokuWiki's media manager.
The app icon comes from `assets/icon.png`; the wiki branding files are in `assets/branding/`.

## Updates
- On launch (at most once per 24 h) ArchiveWiki checks DokuWiki's `stable` VERSION and the newest PHP patch release over HTTPS. Offline or failing checks are silent.
- If a newer release exists you get a dialog: **Update / Later / Skip This Version**. Nothing happens without your click.
- On Update: it downloads the official release, unpacks and verifies it, writes a pre-update zip to the active wiki's backup folder
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
  The build script downloads the pinned version from this repo's Releases. Run the workflow to publish a newer build, then update
  `dependencies.lock` to select it. The repo must be public for the app and build script to download the release.
- The app is ad-hoc signed, which is fine for a locally built app. If you copy it to another Mac, right-click → Open.
