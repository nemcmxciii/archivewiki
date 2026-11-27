#[cfg(not(target_os = "macos"))]
compile_error!("ArchiveWiki targets macOS only.");

mod snapshot;
mod starter;
mod deuterocanon;
mod archive;

use anyhow::{anyhow, bail, Context, Result};
use archive::{extract_archive, inspect_archive, install_staged_restore, validate_backup_zip};
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
    sync::Mutex,
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
use std::sync::Arc;

static WIKI_IO_LOCK: Mutex<()> = Mutex::new(());
use wry::WebViewBuilder;
use wry::NewWindowResponse;

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
const BRANDING_VERSION: u32 = 8;
const LOGO_DARK_PNG: &[u8] = include_bytes!("../assets/branding/logo_dark.png");
const THEME_CSS: &str = include_str!("theme.css");
const BRANDING_CSS: &str = "\n/* ArchiveWiki branding v2: the logo image includes the wordmark */\n#dokuwiki__header .logo img { height: 64px; width: auto; }\n#dokuwiki__header .logo span { position: absolute; width: 1px; height: 1px; overflow: hidden; clip: rect(0 0 0 0); white-space: nowrap; }\n";
const APP_UI_SCRIPT: &str = r##"
(function () {
  function installToolbar() {
    if (!document.body || document.getElementById("archivewiki-navigation-toolbar")) return;

    var host = document.createElement("div");
    host.id = "archivewiki-navigation-toolbar";
    host.setAttribute("role", "group");
    host.setAttribute("aria-label", "ArchiveWiki navigation");
    host.style.cssText = "position:sticky;top:0;z-index:2147483647;display:block;width:100%;";
    function syncColorScheme() {
      var theme = document.documentElement.getAttribute("data-aw-theme");
      host.style.colorScheme = theme === "light" || theme === "dark" ? theme : "light dark";
    }
    syncColorScheme();

    var shadow = host.attachShadow({ mode: "open" });
    shadow.innerHTML = `
      <style>
        :host { display: block; }
        .toolbar {
          box-sizing: border-box;
          display: flex;
          align-items: center;
          gap: 6px;
          min-height: 48px;
          padding: 6px 12px;
          color: CanvasText;
          background: Canvas;
          border-bottom: 1px solid #8c959f;
          font: 13px -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
        }
        button {
          display: inline-flex;
          align-items: center;
          gap: 7px;
          min-height: 34px;
          padding: 5px 11px;
          border: 1px solid #8c959f;
          border-radius: 6px;
          color: inherit;
          background: Canvas;
          font: inherit;
          cursor: pointer;
        }
        button:hover { background: rgba(127, 127, 127, .12); }
        button:focus-visible { outline: 2px solid #0969da; outline-offset: 2px; }
        svg { width: 16px; height: 16px; fill: none; stroke: currentColor; stroke-width: 1.8; }
      </style>
      <div class="toolbar">
        <button type="button" aria-label="Back" title="Back">
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M7 3 2 8l5 5M2 8h12"/></svg>
          <span>Back</span>
        </button>
        <button type="button" aria-label="Forward" title="Forward">
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="m9 3 5 5-5 5m5-5H2"/></svg>
          <span>Forward</span>
        </button>
        <button type="button" aria-label="Refresh" title="Refresh">
          <svg viewBox="0 0 16 16" aria-hidden="true"><path d="M13.5 5.5A5.5 5.5 0 0 0 3 4L1.5 5.5M1.5 2.5v3h3M2.5 10.5A5.5 5.5 0 0 0 13 12l1.5-1.5m0 3v-3h-3"/></svg>
          <span>Refresh</span>
        </button>
      </div>`;

    var buttons = shadow.querySelectorAll("button");
    buttons[0].addEventListener("click", function () { history.back(); });
    buttons[1].addEventListener("click", function () { history.forward(); });
    buttons[2].addEventListener("click", function () { location.reload(); });
    document.body.insertBefore(host, document.body.firstChild);

    new MutationObserver(syncColorScheme).observe(document.documentElement, {
      attributes: true,
      attributeFilter: ["data-aw-theme"]
    });
  }

  var recentPagesKey = "archivewiki.recent-pages";
  var maxRecentPages = 8;

  function currentPageId() {
    var url = new URL(window.location.href);
    if (!url.pathname.endsWith("/doku.php")) return null;
    var action = url.searchParams.get("do");
    if (action && action !== "show") return null;
    return url.searchParams.get("id") || "start";
  }

  function readRecentPages() {
    try {
      var pages = JSON.parse(window.localStorage.getItem(recentPagesKey) || "[]");
      if (!Array.isArray(pages)) return [];
      return pages.filter(function (page) {
        return page && typeof page.id === "string" && page.id !== "start" &&
          typeof page.title === "string" && page.title.length > 0;
      }).slice(0, maxRecentPages);
    } catch (_) {
      return [];
    }
  }

  function pageTitle(pageId) {
    var heading = document.querySelector("#dokuwiki__content .page h1, #dokuwiki__content h1");
    var title = heading && heading.textContent.trim();
    return title || pageId.split(":").map(function (part) {
      return part.replace(/_/g, " ");
    }).join(" / ");
  }

  function findHeading(page, title) {
    var headings = page.querySelectorAll("h1, h2, h3, h4");
    for (var i = 0; i < headings.length; i++) {
      if (headings[i].textContent.trim() === title) return headings[i];
    }
    return null;
  }

  function renderRecentPages() {
    var content = document.querySelector("#dokuwiki__content");
    if (!content) return;
    var page = content.querySelector(".page") || content;
    Array.prototype.forEach.call(page.querySelectorAll("p"), function (paragraph) {
      if (paragraph.textContent.indexOf("This page does not show a list of recent page titles.") >= 0) {
        paragraph.remove();
      }
    });
    var heading = findHeading(page, "Recent pages");
    if (!heading) {
      heading = document.createElement("h2");
      heading.textContent = "Recent pages";
      var before = findHeading(page, "Find or add a page");
      if (before && before.parentNode) {
        before.parentNode.insertBefore(heading, before);
      } else {
        page.appendChild(heading);
      }
    }

    var section = page.querySelector("#archivewiki-recent-pages");
    if (!section) {
      section = document.createElement("div");
      section.id = "archivewiki-recent-pages";
      heading.parentNode.insertBefore(section, heading.nextSibling);
    }
    while (section.firstChild) section.removeChild(section.firstChild);

    var pages = readRecentPages();
    if (!pages.length) {
      var empty = document.createElement("p");
      empty.textContent = "Pages you visit will appear here.";
      section.appendChild(empty);
      return;
    }

    var list = document.createElement("ul");
    pages.forEach(function (recent) {
      var item = document.createElement("li");
      var link = document.createElement("a");
      var query = new URLSearchParams();
      query.set("id", recent.id);
      link.href = "/doku.php?" + query.toString();
      link.textContent = recent.title;
      item.appendChild(link);
      list.appendChild(item);
    });
    section.appendChild(list);

    var clear = document.createElement("button");
    clear.type = "button";
    clear.textContent = "Clear recent pages";
    clear.addEventListener("click", function () {
      try {
        window.localStorage.removeItem(recentPagesKey);
      } catch (_) {}
      renderRecentPages();
    });
    section.appendChild(clear);
  }

  function installNewPageForm() {
    if (document.getElementById("archivewiki-new-page")) return;
    var content = document.querySelector("#dokuwiki__content .page");
    if (!content) return;
    var heading = findHeading(content, "Find or add a page");
    if (!heading || !heading.parentNode) return;

    var style = document.createElement("style");
    style.id = "archivewiki-new-page-style";
    style.textContent = `
      #archivewiki-new-page { margin: .8em 0 1.2em; }
      #archivewiki-new-page label { display: block; margin-bottom: .3em; font-weight: 600; }
      #archivewiki-new-page input { box-sizing: border-box; width: min(100%, 28rem); min-height: 2.5rem; padding: .45rem .6rem; font: inherit; }
      #archivewiki-new-page button { min-height: 2.4rem; margin-top: .6rem; padding: .4rem .8rem; cursor: pointer; }
      #archivewiki-new-page :focus-visible { outline: 3px solid #0969da; outline-offset: 2px; }
    `;
    document.head.appendChild(style);

    var form = document.createElement("form");
    form.id = "archivewiki-new-page";
    var label = document.createElement("label");
    label.htmlFor = "archivewiki-new-page-title";
    label.textContent = "Page title";
    var input = document.createElement("input");
    input.id = "archivewiki-new-page-title";
    input.type = "text";
    input.maxLength = 120;
    input.required = true;
    input.placeholder = "e.g. Family trip";
    input.setAttribute("aria-describedby", "archivewiki-new-page-help");
    var help = document.createElement("p");
    help.id = "archivewiki-new-page-help";
    help.textContent = "We'll prepare the page name for you. If it already exists, it will open without changes.";
    var button = document.createElement("button");
    button.type = "submit";
    button.textContent = "Create or open page";
    var status = document.createElement("p");
    status.id = "archivewiki-new-page-status";
    status.setAttribute("role", "status");
    status.setAttribute("aria-live", "polite");
    form.appendChild(label);
    form.appendChild(input);
    form.appendChild(help);
    form.appendChild(button);
    form.appendChild(status);
    heading.parentNode.insertBefore(form, heading.nextSibling);

    form.addEventListener("submit", async function (event) {
      event.preventDefault();
      if (!input.value.trim()) {
        status.textContent = "Enter a page title.";
        input.focus();
        return;
      }

      button.disabled = true;
      status.textContent = "Preparing the page…";
      try {
        var response = await fetch("/_archivewiki/page-check", {
          method: "POST",
          credentials: "same-origin",
          headers: { "Content-Type": "application/x-www-form-urlencoded;charset=UTF-8" },
          body: new URLSearchParams({ title: input.value.trim() })
        });
        var result = await response.json();
        if (!response.ok) {
          throw new Error(typeof result.error === "string" ? result.error : "The page name could not be used.");
        }
        if (typeof result.id !== "string" || !result.id) throw new Error("Enter a title with letters or numbers.");
        if (typeof result.exists !== "boolean") throw new Error("The page could not be checked. Please try again.");

        var query = new URLSearchParams({ id: result.id });
        if (!result.exists) query.set("do", "edit");
        window.location.assign("/doku.php?" + query.toString());
      } catch (error) {
        status.textContent = error.message || "Could not prepare the page. Please try again.";
        button.disabled = false;
      }
    });
  }

  function initializeRecentPages() {
    var pageId = currentPageId();
    if (pageId === null) return;
    if (pageId === "start") {
      renderRecentPages();
      installNewPageForm();
      return;
    }

    var pages = readRecentPages().filter(function (page) { return page.id !== pageId; });
    pages.unshift({ id: pageId, title: pageTitle(pageId) });
    try {
      window.localStorage.setItem(recentPagesKey, JSON.stringify(pages.slice(0, maxRecentPages)));
    } catch (_) {}
  }

  function initializeAppUi() {
    installToolbar();
    initializeRecentPages();
  }

  if (document.readyState === "loading") {
    document.addEventListener("DOMContentLoaded", initializeAppUi, { once: true });
  } else {
    initializeAppUi();
  }
})();
"##;
const STARTUP_HTML: &str = r##"<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Welcome to ArchiveWiki</title>
<style>
  :root { color-scheme: light dark; font: 16px -apple-system, BlinkMacSystemFont, sans-serif; }
  body { margin: 0; min-height: 100vh; display: grid; place-items: center; background: #f4f6f8; color: #17212b; }
  main { width: min(520px, calc(100% - 40px)); padding: 36px; box-sizing: border-box; border-radius: 16px; background: #fff; box-shadow: 0 12px 40px #17212b20; }
  h1 { margin: 0 0 8px; font-size: 28px; }
  p { color: #526170; line-height: 1.5; margin: 0 0 24px; }
  nav { display: grid; gap: 12px; }
  a { display: block; padding: 14px 16px; border: 1px solid #cbd4dd; border-radius: 10px; color: inherit; text-decoration: none; font-weight: 600; }
  a:hover, a:focus-visible { border-color: #0969da; outline: 2px solid #0969da40; }
  a.primary { color: white; background: #0969da; border-color: #0969da; }
  small { display: block; margin-top: 20px; color: #667583; line-height: 1.4; }
  @media (prefers-color-scheme: dark) {
    body { background: #161b22; color: #e6edf3; }
    main { background: #21262d; box-shadow: 0 12px 40px #0008; }
    p, small { color: #9da7b3; }
    a { border-color: #444c56; }
    a.primary { background: #1f6feb; border-color: #1f6feb; }
  }
</style>
</head>
<body>
<main>
  <h1>Welcome to ArchiveWiki</h1>
  <p>Choose an existing wiki on this Mac, or start a separate one.</p>
  <nav aria-label="Wiki actions">
    <a class="primary" href="archivewiki://startup/login">Log in to an existing wiki</a>
    <a href="archivewiki://startup/create">Create a new wiki</a>
    <a href="archivewiki://startup/import">Import a backup as a new wiki</a>
    <a href="archivewiki://startup/continue">Continue with the last wiki</a>
    <a href="archivewiki://startup/quit">Quit ArchiveWiki</a>
  </nav>
  <small>Existing wiki means a wiki stored on this Mac. Connecting to a hosted wiki is not supported yet.</small>
</main>
</body>
</html>"##;
const JOURNAL_BUTTONS_CSS: &str = r#"
/* ArchiveWiki journal action buttons */
.wrap_aw-journal-actions { display: flex; flex-wrap: wrap; gap: .7rem; margin: 1.2rem 0; }
.wrap_aw-journal-actions p { margin: 0; }
.wrap_aw-journal-actions a { display: inline-block; padding: .65rem 1rem; border: 1px solid #294a67; border-radius: .45rem; background: #365b7a; color: #fff !important; font-weight: 600; text-decoration: none !important; }
.wrap_aw-journal-actions a:hover { background: #294a67; }
.wrap_aw-journal-actions a:focus-visible { outline: 3px solid #82b8e5; outline-offset: 2px; }
"#;
const JOURNAL_BUTTON_CARDS_CSS: &str = r#"
/* ArchiveWiki action cards */
.wrap_aw-journal-actions, .wrap_aw-portfolio-actions { display: flex; flex-wrap: wrap; max-width: 700px; margin: 22px 0 26px; }
.wrap_aw-journal-actions .wrap_aw-journal-action, .wrap_aw-portfolio-actions .wrap_aw-portfolio-action { box-sizing: border-box; flex: 1 1 180px; min-width: 0; margin: 0 12px 12px 0; padding: 16px 18px; border: 1px solid #d7e0e8; border-left: 4px solid #547b9d; border-radius: 10px; background: #f6f8fa; transition: border-color .16s ease, background-color .16s ease; }
.wrap_aw-journal-actions .wrap_aw-journal-action p, .wrap_aw-portfolio-actions .wrap_aw-portfolio-action p { margin: 4px 0 0; color: #596b7b; font-size: .92em; }
.wrap_aw-journal-actions .wrap_aw-journal-action a, .wrap_aw-portfolio-actions .wrap_aw-portfolio-action a { display: block; padding: 0; border: 0; border-radius: 0; border-bottom: 0 !important; background: transparent; color: #294a67 !important; font-size: 1.08em; font-weight: 650; text-decoration: none !important; }
.wrap_aw-journal-actions .wrap_aw-journal-action a::after, .wrap_aw-portfolio-actions .wrap_aw-portfolio-action a::after { content: "  ›"; color: #547b9d; }
.wrap_aw-journal-actions .wrap_aw-journal-action:hover, .wrap_aw-portfolio-actions .wrap_aw-portfolio-action:hover { border-color: #9db3c6; border-left-color: #365b7a; background: #eef3f7; }
.wrap_aw-journal-actions .wrap_aw-journal-action a:focus-visible, .wrap_aw-portfolio-actions .wrap_aw-portfolio-action a:focus-visible { outline: 3px solid #82b8e5; outline-offset: 3px; }
@media (prefers-reduced-motion: reduce) { .wrap_aw-journal-actions .wrap_aw-journal-action, .wrap_aw-portfolio-actions .wrap_aw-portfolio-action { transition: none; } }
"#;
const HOME_ACTION_CARDS_CSS: &str = r#"
/* ArchiveWiki home action cards */
.wrap_aw-home-actions { display: flex; flex-wrap: wrap; gap: .75rem; max-width: 42rem; margin: 1.2rem 0 1.6rem; }
.wrap_aw-home-actions .wrap_aw-home-action { box-sizing: border-box; flex: 1 1 13rem; min-width: 0; padding: 1rem 1.1rem; border: 1px solid #d7e0e8; border-left: 4px solid #547b9d; border-radius: .65rem; background: #f6f8fa; transition: border-color .16s ease, background-color .16s ease; }
.wrap_aw-home-actions .wrap_aw-home-action p { margin: .25rem 0 0; color: #596b7b; font-size: .92em; }
.wrap_aw-home-actions .wrap_aw-home-action a { display: block; color: #294a67 !important; font-size: 1.08em; font-weight: 650; text-decoration: none !important; }
.wrap_aw-home-actions .wrap_aw-home-action a::after { content: "  ›"; color: #547b9d; }
.wrap_aw-home-actions .wrap_aw-home-action:hover { border-color: #9db3c6; border-left-color: #365b7a; background: #eef3f7; }
.wrap_aw-home-actions .wrap_aw-home-action a:focus-visible { outline: 3px solid #82b8e5; outline-offset: 3px; }
@media (prefers-reduced-motion: reduce) { .wrap_aw-home-actions .wrap_aw-home-action { transition: none; } }
"#;
// Version 5 briefly shipped this invalid nested min()/minmax() grid rule. Strip it before adding the compatible
// flex layout so an affected existing wiki can recover on its next launch.
const BROKEN_JOURNAL_BUTTON_CARDS_CSS: &str = r#"
/* ArchiveWiki journal action cards v2 */
.wrap_aw-journal-actions { display: grid; grid-template-columns: repeat(auto-fit, minmax(min(100%, 15rem), 1fr)); gap: .85rem; max-width: 42rem; margin: 1.4rem 0 1.6rem; }
.wrap_aw-journal-actions .wrap_aw-journal-action { box-sizing: border-box; min-width: 0; padding: 1rem 1.1rem; border: 1px solid #d7e0e8; border-left: 4px solid #547b9d; border-radius: .65rem; background: #f6f8fa; transition: border-color .16s ease, background-color .16s ease, transform .16s ease; }
.wrap_aw-journal-actions .wrap_aw-journal-action p { margin: .25rem 0 0; color: #596b7b; font-size: .92em; }
.wrap_aw-journal-actions .wrap_aw-journal-action a { display: block; padding: 0; border: 0; border-radius: 0; background: transparent; color: #294a67 !important; font-size: 1.08em; font-weight: 650; text-decoration: none !important; }
.wrap_aw-journal-actions .wrap_aw-journal-action a::after { content: "  ›"; color: #547b9d; }
.wrap_aw-journal-actions .wrap_aw-journal-action:hover { border-color: #9db3c6; border-left-color: #365b7a; background: #eef3f7; transform: translateY(-1px); }
.wrap_aw-journal-actions .wrap_aw-journal-action a:focus-visible { outline: 3px solid #82b8e5; outline-offset: 3px; }
@media (prefers-reduced-motion: reduce) { .wrap_aw-journal-actions .wrap_aw-journal-action { transition: none; } }
"#;

// Starter structure, installed on first run; new pages are added and untouched built-in pages may be refreshed.
const STARTER_VERSION: u32 = 28;
const STARTER: &[(&str, &str)] = &[
    ("data/pages/start.txt", include_str!("starter/start.txt")),
    ("data/pages/sidebar.txt", include_str!("starter/sidebar.txt")),
    ("data/pages/journal/start.txt", include_str!("starter/journal_start.txt")),
    ("data/pages/journal/_template.txt", include_str!("starter/journal_template.txt")),
    ("data/pages/genealogy/start.txt", include_str!("starter/genealogy_start.txt")),
    ("data/pages/genealogy/_template.txt", include_str!("starter/genealogy_template.txt")),
    ("data/pages/genealogy/chart.txt", include_str!("starter/genealogy_chart.txt")),
    ("data/pages/genealogy/chart_blank.txt", include_str!("starter/genealogy_chart_blank.txt")),
    ("data/pages/genealogy/families/start.txt", include_str!("starter/genealogy_families_start.txt")),
    ("data/pages/portfolio/start.txt", include_str!("starter/portfolio_start.txt")),
    ("data/pages/portfolio/residential.txt", include_str!("starter/portfolio_residential.txt")),
    ("data/pages/portfolio/residential/entry_template.txt", include_str!("starter/portfolio_residential_entry_template.txt")),
    ("data/pages/portfolio/employment.txt", include_str!("starter/portfolio_employment.txt")),
    ("data/pages/portfolio/employment/entry_template.txt", include_str!("starter/portfolio_employment_entry_template.txt")),
    ("data/pages/portfolio/credentials.txt", include_str!("starter/portfolio_credentials.txt")),
    ("data/pages/portfolio/credentials/entry_template.txt", include_str!("starter/portfolio_credentials_entry_template.txt")),
    ("data/pages/reference/start.txt", include_str!("starter/reference_start.txt")),
    ("data/pages/bible/genesis/1.txt", include_str!("starter/bible/genesis/1.txt")),
    ("data/pages/bible/genesis/2.txt", include_str!("starter/bible/genesis/2.txt")),
    ("data/pages/bible/genesis/3.txt", include_str!("starter/bible/genesis/3.txt")),
    ("data/pages/bible/genesis/4.txt", include_str!("starter/bible/genesis/4.txt")),
    ("data/pages/bible/genesis/5.txt", include_str!("starter/bible/genesis/5.txt")),
    ("data/pages/bible/genesis/6.txt", include_str!("starter/bible/genesis/6.txt")),
    ("data/pages/bible/genesis/7.txt", include_str!("starter/bible/genesis/7.txt")),
    ("data/pages/bible/genesis/8.txt", include_str!("starter/bible/genesis/8.txt")),
    ("data/pages/bible/genesis/9.txt", include_str!("starter/bible/genesis/9.txt")),
    ("data/pages/bible/genesis/10.txt", include_str!("starter/bible/genesis/10.txt")),
    ("data/pages/bible/genesis/11.txt", include_str!("starter/bible/genesis/11.txt")),
    ("data/pages/bible/genesis/12.txt", include_str!("starter/bible/genesis/12.txt")),
    ("data/pages/bible/genesis/13.txt", include_str!("starter/bible/genesis/13.txt")),
    ("data/pages/bible/genesis/14.txt", include_str!("starter/bible/genesis/14.txt")),
    ("data/pages/bible/genesis/15.txt", include_str!("starter/bible/genesis/15.txt")),
    ("data/pages/bible/genesis/16.txt", include_str!("starter/bible/genesis/16.txt")),
    ("data/pages/bible/genesis/17.txt", include_str!("starter/bible/genesis/17.txt")),
    ("data/pages/bible/genesis/18.txt", include_str!("starter/bible/genesis/18.txt")),
    ("data/pages/bible/genesis/19.txt", include_str!("starter/bible/genesis/19.txt")),
    ("data/pages/bible/genesis/20.txt", include_str!("starter/bible/genesis/20.txt")),
    ("data/pages/bible/genesis/21.txt", include_str!("starter/bible/genesis/21.txt")),
    ("data/pages/bible/genesis/22.txt", include_str!("starter/bible/genesis/22.txt")),
    ("data/pages/bible/genesis/23.txt", include_str!("starter/bible/genesis/23.txt")),
    ("data/pages/bible/genesis/24.txt", include_str!("starter/bible/genesis/24.txt")),
    ("data/pages/bible/genesis/25.txt", include_str!("starter/bible/genesis/25.txt")),
    ("data/pages/bible/genesis/26.txt", include_str!("starter/bible/genesis/26.txt")),
    ("data/pages/bible/genesis/27.txt", include_str!("starter/bible/genesis/27.txt")),
    ("data/pages/bible/genesis/28.txt", include_str!("starter/bible/genesis/28.txt")),
    ("data/pages/bible/genesis/29.txt", include_str!("starter/bible/genesis/29.txt")),
    ("data/pages/bible/genesis/30.txt", include_str!("starter/bible/genesis/30.txt")),
    ("data/pages/bible/genesis/31.txt", include_str!("starter/bible/genesis/31.txt")),
    ("data/pages/bible/genesis/32.txt", include_str!("starter/bible/genesis/32.txt")),
    ("data/pages/bible/genesis/33.txt", include_str!("starter/bible/genesis/33.txt")),
    ("data/pages/bible/genesis/34.txt", include_str!("starter/bible/genesis/34.txt")),
    ("data/pages/bible/genesis/35.txt", include_str!("starter/bible/genesis/35.txt")),
    ("data/pages/bible/genesis/36.txt", include_str!("starter/bible/genesis/36.txt")),
    ("data/pages/bible/genesis/37.txt", include_str!("starter/bible/genesis/37.txt")),
    ("data/pages/bible/genesis/38.txt", include_str!("starter/bible/genesis/38.txt")),
    ("data/pages/bible/genesis/39.txt", include_str!("starter/bible/genesis/39.txt")),
    ("data/pages/bible/genesis/40.txt", include_str!("starter/bible/genesis/40.txt")),
    ("data/pages/bible/genesis/41.txt", include_str!("starter/bible/genesis/41.txt")),
    ("data/pages/bible/genesis/42.txt", include_str!("starter/bible/genesis/42.txt")),
    ("data/pages/bible/genesis/43.txt", include_str!("starter/bible/genesis/43.txt")),
    ("data/pages/bible/genesis/44.txt", include_str!("starter/bible/genesis/44.txt")),
    ("data/pages/bible/genesis/45.txt", include_str!("starter/bible/genesis/45.txt")),
    ("data/pages/bible/genesis/46.txt", include_str!("starter/bible/genesis/46.txt")),
    ("data/pages/bible/genesis/47.txt", include_str!("starter/bible/genesis/47.txt")),
    ("data/pages/bible/genesis/48.txt", include_str!("starter/bible/genesis/48.txt")),
    ("data/pages/bible/genesis/49.txt", include_str!("starter/bible/genesis/49.txt")),
    ("data/pages/bible/genesis/50.txt", include_str!("starter/bible/genesis/50.txt")),
    ("data/pages/bible/genesis/start.txt", include_str!("starter/bible/genesis/start.txt")),
    ("data/pages/bible/exodus/1.txt", include_str!("starter/bible/exodus/1.txt")),
    ("data/pages/bible/exodus/2.txt", include_str!("starter/bible/exodus/2.txt")),
    ("data/pages/bible/exodus/3.txt", include_str!("starter/bible/exodus/3.txt")),
    ("data/pages/bible/exodus/4.txt", include_str!("starter/bible/exodus/4.txt")),
    ("data/pages/bible/exodus/5.txt", include_str!("starter/bible/exodus/5.txt")),
    ("data/pages/bible/exodus/6.txt", include_str!("starter/bible/exodus/6.txt")),
    ("data/pages/bible/exodus/7.txt", include_str!("starter/bible/exodus/7.txt")),
    ("data/pages/bible/exodus/8.txt", include_str!("starter/bible/exodus/8.txt")),
    ("data/pages/bible/exodus/9.txt", include_str!("starter/bible/exodus/9.txt")),
    ("data/pages/bible/exodus/10.txt", include_str!("starter/bible/exodus/10.txt")),
    ("data/pages/bible/exodus/11.txt", include_str!("starter/bible/exodus/11.txt")),
    ("data/pages/bible/exodus/12.txt", include_str!("starter/bible/exodus/12.txt")),
    ("data/pages/bible/exodus/13.txt", include_str!("starter/bible/exodus/13.txt")),
    ("data/pages/bible/exodus/14.txt", include_str!("starter/bible/exodus/14.txt")),
    ("data/pages/bible/exodus/15.txt", include_str!("starter/bible/exodus/15.txt")),
    ("data/pages/bible/exodus/16.txt", include_str!("starter/bible/exodus/16.txt")),
    ("data/pages/bible/exodus/17.txt", include_str!("starter/bible/exodus/17.txt")),
    ("data/pages/bible/exodus/18.txt", include_str!("starter/bible/exodus/18.txt")),
    ("data/pages/bible/exodus/19.txt", include_str!("starter/bible/exodus/19.txt")),
    ("data/pages/bible/exodus/20.txt", include_str!("starter/bible/exodus/20.txt")),
    ("data/pages/bible/exodus/21.txt", include_str!("starter/bible/exodus/21.txt")),
    ("data/pages/bible/exodus/22.txt", include_str!("starter/bible/exodus/22.txt")),
    ("data/pages/bible/exodus/23.txt", include_str!("starter/bible/exodus/23.txt")),
    ("data/pages/bible/exodus/24.txt", include_str!("starter/bible/exodus/24.txt")),
    ("data/pages/bible/exodus/25.txt", include_str!("starter/bible/exodus/25.txt")),
    ("data/pages/bible/exodus/26.txt", include_str!("starter/bible/exodus/26.txt")),
    ("data/pages/bible/exodus/27.txt", include_str!("starter/bible/exodus/27.txt")),
    ("data/pages/bible/exodus/28.txt", include_str!("starter/bible/exodus/28.txt")),
    ("data/pages/bible/exodus/29.txt", include_str!("starter/bible/exodus/29.txt")),
    ("data/pages/bible/exodus/30.txt", include_str!("starter/bible/exodus/30.txt")),
    ("data/pages/bible/exodus/31.txt", include_str!("starter/bible/exodus/31.txt")),
    ("data/pages/bible/exodus/32.txt", include_str!("starter/bible/exodus/32.txt")),
    ("data/pages/bible/exodus/33.txt", include_str!("starter/bible/exodus/33.txt")),
    ("data/pages/bible/exodus/34.txt", include_str!("starter/bible/exodus/34.txt")),
    ("data/pages/bible/exodus/35.txt", include_str!("starter/bible/exodus/35.txt")),
    ("data/pages/bible/exodus/36.txt", include_str!("starter/bible/exodus/36.txt")),
    ("data/pages/bible/exodus/37.txt", include_str!("starter/bible/exodus/37.txt")),
    ("data/pages/bible/exodus/38.txt", include_str!("starter/bible/exodus/38.txt")),
    ("data/pages/bible/exodus/39.txt", include_str!("starter/bible/exodus/39.txt")),
    ("data/pages/bible/exodus/40.txt", include_str!("starter/bible/exodus/40.txt")),
    ("data/pages/bible/exodus/start.txt", include_str!("starter/bible/exodus/start.txt")),
    ("data/pages/bible/leviticus/1.txt", include_str!("starter/bible/leviticus/1.txt")),
    ("data/pages/bible/leviticus/2.txt", include_str!("starter/bible/leviticus/2.txt")),
    ("data/pages/bible/leviticus/3.txt", include_str!("starter/bible/leviticus/3.txt")),
    ("data/pages/bible/leviticus/4.txt", include_str!("starter/bible/leviticus/4.txt")),
    ("data/pages/bible/leviticus/5.txt", include_str!("starter/bible/leviticus/5.txt")),
    ("data/pages/bible/leviticus/6.txt", include_str!("starter/bible/leviticus/6.txt")),
    ("data/pages/bible/leviticus/7.txt", include_str!("starter/bible/leviticus/7.txt")),
    ("data/pages/bible/leviticus/8.txt", include_str!("starter/bible/leviticus/8.txt")),
    ("data/pages/bible/leviticus/9.txt", include_str!("starter/bible/leviticus/9.txt")),
    ("data/pages/bible/leviticus/10.txt", include_str!("starter/bible/leviticus/10.txt")),
    ("data/pages/bible/leviticus/11.txt", include_str!("starter/bible/leviticus/11.txt")),
    ("data/pages/bible/leviticus/12.txt", include_str!("starter/bible/leviticus/12.txt")),
    ("data/pages/bible/leviticus/13.txt", include_str!("starter/bible/leviticus/13.txt")),
    ("data/pages/bible/leviticus/14.txt", include_str!("starter/bible/leviticus/14.txt")),
    ("data/pages/bible/leviticus/15.txt", include_str!("starter/bible/leviticus/15.txt")),
    ("data/pages/bible/leviticus/16.txt", include_str!("starter/bible/leviticus/16.txt")),
    ("data/pages/bible/leviticus/17.txt", include_str!("starter/bible/leviticus/17.txt")),
    ("data/pages/bible/leviticus/18.txt", include_str!("starter/bible/leviticus/18.txt")),
    ("data/pages/bible/leviticus/19.txt", include_str!("starter/bible/leviticus/19.txt")),
    ("data/pages/bible/leviticus/20.txt", include_str!("starter/bible/leviticus/20.txt")),
    ("data/pages/bible/leviticus/21.txt", include_str!("starter/bible/leviticus/21.txt")),
    ("data/pages/bible/leviticus/22.txt", include_str!("starter/bible/leviticus/22.txt")),
    ("data/pages/bible/leviticus/23.txt", include_str!("starter/bible/leviticus/23.txt")),
    ("data/pages/bible/leviticus/24.txt", include_str!("starter/bible/leviticus/24.txt")),
    ("data/pages/bible/leviticus/25.txt", include_str!("starter/bible/leviticus/25.txt")),
    ("data/pages/bible/leviticus/26.txt", include_str!("starter/bible/leviticus/26.txt")),
    ("data/pages/bible/leviticus/27.txt", include_str!("starter/bible/leviticus/27.txt")),
    ("data/pages/bible/leviticus/start.txt", include_str!("starter/bible/leviticus/start.txt")),
    ("data/pages/bible/numbers/1.txt", include_str!("starter/bible/numbers/1.txt")),
    ("data/pages/bible/numbers/2.txt", include_str!("starter/bible/numbers/2.txt")),
    ("data/pages/bible/numbers/3.txt", include_str!("starter/bible/numbers/3.txt")),
    ("data/pages/bible/numbers/4.txt", include_str!("starter/bible/numbers/4.txt")),
    ("data/pages/bible/numbers/5.txt", include_str!("starter/bible/numbers/5.txt")),
    ("data/pages/bible/numbers/6.txt", include_str!("starter/bible/numbers/6.txt")),
    ("data/pages/bible/numbers/7.txt", include_str!("starter/bible/numbers/7.txt")),
    ("data/pages/bible/numbers/8.txt", include_str!("starter/bible/numbers/8.txt")),
    ("data/pages/bible/numbers/9.txt", include_str!("starter/bible/numbers/9.txt")),
    ("data/pages/bible/numbers/10.txt", include_str!("starter/bible/numbers/10.txt")),
    ("data/pages/bible/numbers/11.txt", include_str!("starter/bible/numbers/11.txt")),
    ("data/pages/bible/numbers/12.txt", include_str!("starter/bible/numbers/12.txt")),
    ("data/pages/bible/numbers/13.txt", include_str!("starter/bible/numbers/13.txt")),
    ("data/pages/bible/numbers/14.txt", include_str!("starter/bible/numbers/14.txt")),
    ("data/pages/bible/numbers/15.txt", include_str!("starter/bible/numbers/15.txt")),
    ("data/pages/bible/numbers/16.txt", include_str!("starter/bible/numbers/16.txt")),
    ("data/pages/bible/numbers/17.txt", include_str!("starter/bible/numbers/17.txt")),
    ("data/pages/bible/numbers/18.txt", include_str!("starter/bible/numbers/18.txt")),
    ("data/pages/bible/numbers/19.txt", include_str!("starter/bible/numbers/19.txt")),
    ("data/pages/bible/numbers/20.txt", include_str!("starter/bible/numbers/20.txt")),
    ("data/pages/bible/numbers/21.txt", include_str!("starter/bible/numbers/21.txt")),
    ("data/pages/bible/numbers/22.txt", include_str!("starter/bible/numbers/22.txt")),
    ("data/pages/bible/numbers/23.txt", include_str!("starter/bible/numbers/23.txt")),
    ("data/pages/bible/numbers/24.txt", include_str!("starter/bible/numbers/24.txt")),
    ("data/pages/bible/numbers/25.txt", include_str!("starter/bible/numbers/25.txt")),
    ("data/pages/bible/numbers/26.txt", include_str!("starter/bible/numbers/26.txt")),
    ("data/pages/bible/numbers/27.txt", include_str!("starter/bible/numbers/27.txt")),
    ("data/pages/bible/numbers/28.txt", include_str!("starter/bible/numbers/28.txt")),
    ("data/pages/bible/numbers/29.txt", include_str!("starter/bible/numbers/29.txt")),
    ("data/pages/bible/numbers/30.txt", include_str!("starter/bible/numbers/30.txt")),
    ("data/pages/bible/numbers/31.txt", include_str!("starter/bible/numbers/31.txt")),
    ("data/pages/bible/numbers/32.txt", include_str!("starter/bible/numbers/32.txt")),
    ("data/pages/bible/numbers/33.txt", include_str!("starter/bible/numbers/33.txt")),
    ("data/pages/bible/numbers/34.txt", include_str!("starter/bible/numbers/34.txt")),
    ("data/pages/bible/numbers/35.txt", include_str!("starter/bible/numbers/35.txt")),
    ("data/pages/bible/numbers/36.txt", include_str!("starter/bible/numbers/36.txt")),
    ("data/pages/bible/numbers/start.txt", include_str!("starter/bible/numbers/start.txt")),
    ("data/pages/bible/deuteronomy/1.txt", include_str!("starter/bible/deuteronomy/1.txt")),
    ("data/pages/bible/deuteronomy/2.txt", include_str!("starter/bible/deuteronomy/2.txt")),
    ("data/pages/bible/deuteronomy/3.txt", include_str!("starter/bible/deuteronomy/3.txt")),
    ("data/pages/bible/deuteronomy/4.txt", include_str!("starter/bible/deuteronomy/4.txt")),
    ("data/pages/bible/deuteronomy/5.txt", include_str!("starter/bible/deuteronomy/5.txt")),
    ("data/pages/bible/deuteronomy/6.txt", include_str!("starter/bible/deuteronomy/6.txt")),
    ("data/pages/bible/deuteronomy/7.txt", include_str!("starter/bible/deuteronomy/7.txt")),
    ("data/pages/bible/deuteronomy/8.txt", include_str!("starter/bible/deuteronomy/8.txt")),
    ("data/pages/bible/deuteronomy/9.txt", include_str!("starter/bible/deuteronomy/9.txt")),
    ("data/pages/bible/deuteronomy/10.txt", include_str!("starter/bible/deuteronomy/10.txt")),
    ("data/pages/bible/deuteronomy/11.txt", include_str!("starter/bible/deuteronomy/11.txt")),
    ("data/pages/bible/deuteronomy/12.txt", include_str!("starter/bible/deuteronomy/12.txt")),
    ("data/pages/bible/deuteronomy/13.txt", include_str!("starter/bible/deuteronomy/13.txt")),
    ("data/pages/bible/deuteronomy/14.txt", include_str!("starter/bible/deuteronomy/14.txt")),
    ("data/pages/bible/deuteronomy/15.txt", include_str!("starter/bible/deuteronomy/15.txt")),
    ("data/pages/bible/deuteronomy/16.txt", include_str!("starter/bible/deuteronomy/16.txt")),
    ("data/pages/bible/deuteronomy/17.txt", include_str!("starter/bible/deuteronomy/17.txt")),
    ("data/pages/bible/deuteronomy/18.txt", include_str!("starter/bible/deuteronomy/18.txt")),
    ("data/pages/bible/deuteronomy/19.txt", include_str!("starter/bible/deuteronomy/19.txt")),
    ("data/pages/bible/deuteronomy/20.txt", include_str!("starter/bible/deuteronomy/20.txt")),
    ("data/pages/bible/deuteronomy/21.txt", include_str!("starter/bible/deuteronomy/21.txt")),
    ("data/pages/bible/deuteronomy/22.txt", include_str!("starter/bible/deuteronomy/22.txt")),
    ("data/pages/bible/deuteronomy/23.txt", include_str!("starter/bible/deuteronomy/23.txt")),
    ("data/pages/bible/deuteronomy/24.txt", include_str!("starter/bible/deuteronomy/24.txt")),
    ("data/pages/bible/deuteronomy/25.txt", include_str!("starter/bible/deuteronomy/25.txt")),
    ("data/pages/bible/deuteronomy/26.txt", include_str!("starter/bible/deuteronomy/26.txt")),
    ("data/pages/bible/deuteronomy/27.txt", include_str!("starter/bible/deuteronomy/27.txt")),
    ("data/pages/bible/deuteronomy/28.txt", include_str!("starter/bible/deuteronomy/28.txt")),
    ("data/pages/bible/deuteronomy/29.txt", include_str!("starter/bible/deuteronomy/29.txt")),
    ("data/pages/bible/deuteronomy/30.txt", include_str!("starter/bible/deuteronomy/30.txt")),
    ("data/pages/bible/deuteronomy/31.txt", include_str!("starter/bible/deuteronomy/31.txt")),
    ("data/pages/bible/deuteronomy/32.txt", include_str!("starter/bible/deuteronomy/32.txt")),
    ("data/pages/bible/deuteronomy/33.txt", include_str!("starter/bible/deuteronomy/33.txt")),
    ("data/pages/bible/deuteronomy/34.txt", include_str!("starter/bible/deuteronomy/34.txt")),
    ("data/pages/bible/deuteronomy/start.txt", include_str!("starter/bible/deuteronomy/start.txt")),
    ("data/pages/bible/joshua/1.txt", include_str!("starter/bible/joshua/1.txt")),
    ("data/pages/bible/joshua/2.txt", include_str!("starter/bible/joshua/2.txt")),
    ("data/pages/bible/joshua/3.txt", include_str!("starter/bible/joshua/3.txt")),
    ("data/pages/bible/joshua/4.txt", include_str!("starter/bible/joshua/4.txt")),
    ("data/pages/bible/joshua/5.txt", include_str!("starter/bible/joshua/5.txt")),
    ("data/pages/bible/joshua/6.txt", include_str!("starter/bible/joshua/6.txt")),
    ("data/pages/bible/joshua/7.txt", include_str!("starter/bible/joshua/7.txt")),
    ("data/pages/bible/joshua/8.txt", include_str!("starter/bible/joshua/8.txt")),
    ("data/pages/bible/joshua/9.txt", include_str!("starter/bible/joshua/9.txt")),
    ("data/pages/bible/joshua/10.txt", include_str!("starter/bible/joshua/10.txt")),
    ("data/pages/bible/joshua/11.txt", include_str!("starter/bible/joshua/11.txt")),
    ("data/pages/bible/joshua/12.txt", include_str!("starter/bible/joshua/12.txt")),
    ("data/pages/bible/joshua/13.txt", include_str!("starter/bible/joshua/13.txt")),
    ("data/pages/bible/joshua/14.txt", include_str!("starter/bible/joshua/14.txt")),
    ("data/pages/bible/joshua/15.txt", include_str!("starter/bible/joshua/15.txt")),
    ("data/pages/bible/joshua/16.txt", include_str!("starter/bible/joshua/16.txt")),
    ("data/pages/bible/joshua/17.txt", include_str!("starter/bible/joshua/17.txt")),
    ("data/pages/bible/joshua/18.txt", include_str!("starter/bible/joshua/18.txt")),
    ("data/pages/bible/joshua/19.txt", include_str!("starter/bible/joshua/19.txt")),
    ("data/pages/bible/joshua/20.txt", include_str!("starter/bible/joshua/20.txt")),
    ("data/pages/bible/joshua/21.txt", include_str!("starter/bible/joshua/21.txt")),
    ("data/pages/bible/joshua/22.txt", include_str!("starter/bible/joshua/22.txt")),
    ("data/pages/bible/joshua/23.txt", include_str!("starter/bible/joshua/23.txt")),
    ("data/pages/bible/joshua/24.txt", include_str!("starter/bible/joshua/24.txt")),
    ("data/pages/bible/joshua/start.txt", include_str!("starter/bible/joshua/start.txt")),
    ("data/pages/bible/judges/1.txt", include_str!("starter/bible/judges/1.txt")),
    ("data/pages/bible/judges/2.txt", include_str!("starter/bible/judges/2.txt")),
    ("data/pages/bible/judges/3.txt", include_str!("starter/bible/judges/3.txt")),
    ("data/pages/bible/judges/4.txt", include_str!("starter/bible/judges/4.txt")),
    ("data/pages/bible/judges/5.txt", include_str!("starter/bible/judges/5.txt")),
    ("data/pages/bible/judges/6.txt", include_str!("starter/bible/judges/6.txt")),
    ("data/pages/bible/judges/7.txt", include_str!("starter/bible/judges/7.txt")),
    ("data/pages/bible/judges/8.txt", include_str!("starter/bible/judges/8.txt")),
    ("data/pages/bible/judges/9.txt", include_str!("starter/bible/judges/9.txt")),
    ("data/pages/bible/judges/10.txt", include_str!("starter/bible/judges/10.txt")),
    ("data/pages/bible/judges/11.txt", include_str!("starter/bible/judges/11.txt")),
    ("data/pages/bible/judges/12.txt", include_str!("starter/bible/judges/12.txt")),
    ("data/pages/bible/judges/13.txt", include_str!("starter/bible/judges/13.txt")),
    ("data/pages/bible/judges/14.txt", include_str!("starter/bible/judges/14.txt")),
    ("data/pages/bible/judges/15.txt", include_str!("starter/bible/judges/15.txt")),
    ("data/pages/bible/judges/16.txt", include_str!("starter/bible/judges/16.txt")),
    ("data/pages/bible/judges/17.txt", include_str!("starter/bible/judges/17.txt")),
    ("data/pages/bible/judges/18.txt", include_str!("starter/bible/judges/18.txt")),
    ("data/pages/bible/judges/19.txt", include_str!("starter/bible/judges/19.txt")),
    ("data/pages/bible/judges/20.txt", include_str!("starter/bible/judges/20.txt")),
    ("data/pages/bible/judges/21.txt", include_str!("starter/bible/judges/21.txt")),
    ("data/pages/bible/judges/start.txt", include_str!("starter/bible/judges/start.txt")),
    ("data/pages/bible/ruth/1.txt", include_str!("starter/bible/ruth/1.txt")),
    ("data/pages/bible/ruth/2.txt", include_str!("starter/bible/ruth/2.txt")),
    ("data/pages/bible/ruth/3.txt", include_str!("starter/bible/ruth/3.txt")),
    ("data/pages/bible/ruth/4.txt", include_str!("starter/bible/ruth/4.txt")),
    ("data/pages/bible/ruth/start.txt", include_str!("starter/bible/ruth/start.txt")),
    ("data/pages/bible/1_samuel/1.txt", include_str!("starter/bible/1_samuel/1.txt")),
    ("data/pages/bible/1_samuel/2.txt", include_str!("starter/bible/1_samuel/2.txt")),
    ("data/pages/bible/1_samuel/3.txt", include_str!("starter/bible/1_samuel/3.txt")),
    ("data/pages/bible/1_samuel/4.txt", include_str!("starter/bible/1_samuel/4.txt")),
    ("data/pages/bible/1_samuel/5.txt", include_str!("starter/bible/1_samuel/5.txt")),
    ("data/pages/bible/1_samuel/6.txt", include_str!("starter/bible/1_samuel/6.txt")),
    ("data/pages/bible/1_samuel/7.txt", include_str!("starter/bible/1_samuel/7.txt")),
    ("data/pages/bible/1_samuel/8.txt", include_str!("starter/bible/1_samuel/8.txt")),
    ("data/pages/bible/1_samuel/9.txt", include_str!("starter/bible/1_samuel/9.txt")),
    ("data/pages/bible/1_samuel/10.txt", include_str!("starter/bible/1_samuel/10.txt")),
    ("data/pages/bible/1_samuel/11.txt", include_str!("starter/bible/1_samuel/11.txt")),
    ("data/pages/bible/1_samuel/12.txt", include_str!("starter/bible/1_samuel/12.txt")),
    ("data/pages/bible/1_samuel/13.txt", include_str!("starter/bible/1_samuel/13.txt")),
    ("data/pages/bible/1_samuel/14.txt", include_str!("starter/bible/1_samuel/14.txt")),
    ("data/pages/bible/1_samuel/15.txt", include_str!("starter/bible/1_samuel/15.txt")),
    ("data/pages/bible/1_samuel/16.txt", include_str!("starter/bible/1_samuel/16.txt")),
    ("data/pages/bible/1_samuel/17.txt", include_str!("starter/bible/1_samuel/17.txt")),
    ("data/pages/bible/1_samuel/18.txt", include_str!("starter/bible/1_samuel/18.txt")),
    ("data/pages/bible/1_samuel/19.txt", include_str!("starter/bible/1_samuel/19.txt")),
    ("data/pages/bible/1_samuel/20.txt", include_str!("starter/bible/1_samuel/20.txt")),
    ("data/pages/bible/1_samuel/21.txt", include_str!("starter/bible/1_samuel/21.txt")),
    ("data/pages/bible/1_samuel/22.txt", include_str!("starter/bible/1_samuel/22.txt")),
    ("data/pages/bible/1_samuel/23.txt", include_str!("starter/bible/1_samuel/23.txt")),
    ("data/pages/bible/1_samuel/24.txt", include_str!("starter/bible/1_samuel/24.txt")),
    ("data/pages/bible/1_samuel/25.txt", include_str!("starter/bible/1_samuel/25.txt")),
    ("data/pages/bible/1_samuel/26.txt", include_str!("starter/bible/1_samuel/26.txt")),
    ("data/pages/bible/1_samuel/27.txt", include_str!("starter/bible/1_samuel/27.txt")),
    ("data/pages/bible/1_samuel/28.txt", include_str!("starter/bible/1_samuel/28.txt")),
    ("data/pages/bible/1_samuel/29.txt", include_str!("starter/bible/1_samuel/29.txt")),
    ("data/pages/bible/1_samuel/30.txt", include_str!("starter/bible/1_samuel/30.txt")),
    ("data/pages/bible/1_samuel/31.txt", include_str!("starter/bible/1_samuel/31.txt")),
    ("data/pages/bible/1_samuel/start.txt", include_str!("starter/bible/1_samuel/start.txt")),
    ("data/pages/bible/2_samuel/1.txt", include_str!("starter/bible/2_samuel/1.txt")),
    ("data/pages/bible/2_samuel/2.txt", include_str!("starter/bible/2_samuel/2.txt")),
    ("data/pages/bible/2_samuel/3.txt", include_str!("starter/bible/2_samuel/3.txt")),
    ("data/pages/bible/2_samuel/4.txt", include_str!("starter/bible/2_samuel/4.txt")),
    ("data/pages/bible/2_samuel/5.txt", include_str!("starter/bible/2_samuel/5.txt")),
    ("data/pages/bible/2_samuel/6.txt", include_str!("starter/bible/2_samuel/6.txt")),
    ("data/pages/bible/2_samuel/7.txt", include_str!("starter/bible/2_samuel/7.txt")),
    ("data/pages/bible/2_samuel/8.txt", include_str!("starter/bible/2_samuel/8.txt")),
    ("data/pages/bible/2_samuel/9.txt", include_str!("starter/bible/2_samuel/9.txt")),
    ("data/pages/bible/2_samuel/10.txt", include_str!("starter/bible/2_samuel/10.txt")),
    ("data/pages/bible/2_samuel/11.txt", include_str!("starter/bible/2_samuel/11.txt")),
    ("data/pages/bible/2_samuel/12.txt", include_str!("starter/bible/2_samuel/12.txt")),
    ("data/pages/bible/2_samuel/13.txt", include_str!("starter/bible/2_samuel/13.txt")),
    ("data/pages/bible/2_samuel/14.txt", include_str!("starter/bible/2_samuel/14.txt")),
    ("data/pages/bible/2_samuel/15.txt", include_str!("starter/bible/2_samuel/15.txt")),
    ("data/pages/bible/2_samuel/16.txt", include_str!("starter/bible/2_samuel/16.txt")),
    ("data/pages/bible/2_samuel/17.txt", include_str!("starter/bible/2_samuel/17.txt")),
    ("data/pages/bible/2_samuel/18.txt", include_str!("starter/bible/2_samuel/18.txt")),
    ("data/pages/bible/2_samuel/19.txt", include_str!("starter/bible/2_samuel/19.txt")),
    ("data/pages/bible/2_samuel/20.txt", include_str!("starter/bible/2_samuel/20.txt")),
    ("data/pages/bible/2_samuel/21.txt", include_str!("starter/bible/2_samuel/21.txt")),
    ("data/pages/bible/2_samuel/22.txt", include_str!("starter/bible/2_samuel/22.txt")),
    ("data/pages/bible/2_samuel/23.txt", include_str!("starter/bible/2_samuel/23.txt")),
    ("data/pages/bible/2_samuel/24.txt", include_str!("starter/bible/2_samuel/24.txt")),
    ("data/pages/bible/2_samuel/start.txt", include_str!("starter/bible/2_samuel/start.txt")),
    ("data/pages/bible/1_kings/1.txt", include_str!("starter/bible/1_kings/1.txt")),
    ("data/pages/bible/1_kings/2.txt", include_str!("starter/bible/1_kings/2.txt")),
    ("data/pages/bible/1_kings/3.txt", include_str!("starter/bible/1_kings/3.txt")),
    ("data/pages/bible/1_kings/4.txt", include_str!("starter/bible/1_kings/4.txt")),
    ("data/pages/bible/1_kings/5.txt", include_str!("starter/bible/1_kings/5.txt")),
    ("data/pages/bible/1_kings/6.txt", include_str!("starter/bible/1_kings/6.txt")),
    ("data/pages/bible/1_kings/7.txt", include_str!("starter/bible/1_kings/7.txt")),
    ("data/pages/bible/1_kings/8.txt", include_str!("starter/bible/1_kings/8.txt")),
    ("data/pages/bible/1_kings/9.txt", include_str!("starter/bible/1_kings/9.txt")),
    ("data/pages/bible/1_kings/10.txt", include_str!("starter/bible/1_kings/10.txt")),
    ("data/pages/bible/1_kings/11.txt", include_str!("starter/bible/1_kings/11.txt")),
    ("data/pages/bible/1_kings/12.txt", include_str!("starter/bible/1_kings/12.txt")),
    ("data/pages/bible/1_kings/13.txt", include_str!("starter/bible/1_kings/13.txt")),
    ("data/pages/bible/1_kings/14.txt", include_str!("starter/bible/1_kings/14.txt")),
    ("data/pages/bible/1_kings/15.txt", include_str!("starter/bible/1_kings/15.txt")),
    ("data/pages/bible/1_kings/16.txt", include_str!("starter/bible/1_kings/16.txt")),
    ("data/pages/bible/1_kings/17.txt", include_str!("starter/bible/1_kings/17.txt")),
    ("data/pages/bible/1_kings/18.txt", include_str!("starter/bible/1_kings/18.txt")),
    ("data/pages/bible/1_kings/19.txt", include_str!("starter/bible/1_kings/19.txt")),
    ("data/pages/bible/1_kings/20.txt", include_str!("starter/bible/1_kings/20.txt")),
    ("data/pages/bible/1_kings/21.txt", include_str!("starter/bible/1_kings/21.txt")),
    ("data/pages/bible/1_kings/22.txt", include_str!("starter/bible/1_kings/22.txt")),
    ("data/pages/bible/1_kings/start.txt", include_str!("starter/bible/1_kings/start.txt")),
    ("data/pages/bible/2_kings/1.txt", include_str!("starter/bible/2_kings/1.txt")),
    ("data/pages/bible/2_kings/2.txt", include_str!("starter/bible/2_kings/2.txt")),
    ("data/pages/bible/2_kings/3.txt", include_str!("starter/bible/2_kings/3.txt")),
    ("data/pages/bible/2_kings/4.txt", include_str!("starter/bible/2_kings/4.txt")),
    ("data/pages/bible/2_kings/5.txt", include_str!("starter/bible/2_kings/5.txt")),
    ("data/pages/bible/2_kings/6.txt", include_str!("starter/bible/2_kings/6.txt")),
    ("data/pages/bible/2_kings/7.txt", include_str!("starter/bible/2_kings/7.txt")),
    ("data/pages/bible/2_kings/8.txt", include_str!("starter/bible/2_kings/8.txt")),
    ("data/pages/bible/2_kings/9.txt", include_str!("starter/bible/2_kings/9.txt")),
    ("data/pages/bible/2_kings/10.txt", include_str!("starter/bible/2_kings/10.txt")),
    ("data/pages/bible/2_kings/11.txt", include_str!("starter/bible/2_kings/11.txt")),
    ("data/pages/bible/2_kings/12.txt", include_str!("starter/bible/2_kings/12.txt")),
    ("data/pages/bible/2_kings/13.txt", include_str!("starter/bible/2_kings/13.txt")),
    ("data/pages/bible/2_kings/14.txt", include_str!("starter/bible/2_kings/14.txt")),
    ("data/pages/bible/2_kings/15.txt", include_str!("starter/bible/2_kings/15.txt")),
    ("data/pages/bible/2_kings/16.txt", include_str!("starter/bible/2_kings/16.txt")),
    ("data/pages/bible/2_kings/17.txt", include_str!("starter/bible/2_kings/17.txt")),
    ("data/pages/bible/2_kings/18.txt", include_str!("starter/bible/2_kings/18.txt")),
    ("data/pages/bible/2_kings/19.txt", include_str!("starter/bible/2_kings/19.txt")),
    ("data/pages/bible/2_kings/20.txt", include_str!("starter/bible/2_kings/20.txt")),
    ("data/pages/bible/2_kings/21.txt", include_str!("starter/bible/2_kings/21.txt")),
    ("data/pages/bible/2_kings/22.txt", include_str!("starter/bible/2_kings/22.txt")),
    ("data/pages/bible/2_kings/23.txt", include_str!("starter/bible/2_kings/23.txt")),
    ("data/pages/bible/2_kings/24.txt", include_str!("starter/bible/2_kings/24.txt")),
    ("data/pages/bible/2_kings/25.txt", include_str!("starter/bible/2_kings/25.txt")),
    ("data/pages/bible/2_kings/start.txt", include_str!("starter/bible/2_kings/start.txt")),
    ("data/pages/bible/1_chronicles/1.txt", include_str!("starter/bible/1_chronicles/1.txt")),
    ("data/pages/bible/1_chronicles/2.txt", include_str!("starter/bible/1_chronicles/2.txt")),
    ("data/pages/bible/1_chronicles/3.txt", include_str!("starter/bible/1_chronicles/3.txt")),
    ("data/pages/bible/1_chronicles/4.txt", include_str!("starter/bible/1_chronicles/4.txt")),
    ("data/pages/bible/1_chronicles/5.txt", include_str!("starter/bible/1_chronicles/5.txt")),
    ("data/pages/bible/1_chronicles/6.txt", include_str!("starter/bible/1_chronicles/6.txt")),
    ("data/pages/bible/1_chronicles/7.txt", include_str!("starter/bible/1_chronicles/7.txt")),
    ("data/pages/bible/1_chronicles/8.txt", include_str!("starter/bible/1_chronicles/8.txt")),
    ("data/pages/bible/1_chronicles/9.txt", include_str!("starter/bible/1_chronicles/9.txt")),
    ("data/pages/bible/1_chronicles/10.txt", include_str!("starter/bible/1_chronicles/10.txt")),
    ("data/pages/bible/1_chronicles/11.txt", include_str!("starter/bible/1_chronicles/11.txt")),
    ("data/pages/bible/1_chronicles/12.txt", include_str!("starter/bible/1_chronicles/12.txt")),
    ("data/pages/bible/1_chronicles/13.txt", include_str!("starter/bible/1_chronicles/13.txt")),
    ("data/pages/bible/1_chronicles/14.txt", include_str!("starter/bible/1_chronicles/14.txt")),
    ("data/pages/bible/1_chronicles/15.txt", include_str!("starter/bible/1_chronicles/15.txt")),
    ("data/pages/bible/1_chronicles/16.txt", include_str!("starter/bible/1_chronicles/16.txt")),
    ("data/pages/bible/1_chronicles/17.txt", include_str!("starter/bible/1_chronicles/17.txt")),
    ("data/pages/bible/1_chronicles/18.txt", include_str!("starter/bible/1_chronicles/18.txt")),
    ("data/pages/bible/1_chronicles/19.txt", include_str!("starter/bible/1_chronicles/19.txt")),
    ("data/pages/bible/1_chronicles/20.txt", include_str!("starter/bible/1_chronicles/20.txt")),
    ("data/pages/bible/1_chronicles/21.txt", include_str!("starter/bible/1_chronicles/21.txt")),
    ("data/pages/bible/1_chronicles/22.txt", include_str!("starter/bible/1_chronicles/22.txt")),
    ("data/pages/bible/1_chronicles/23.txt", include_str!("starter/bible/1_chronicles/23.txt")),
    ("data/pages/bible/1_chronicles/24.txt", include_str!("starter/bible/1_chronicles/24.txt")),
    ("data/pages/bible/1_chronicles/25.txt", include_str!("starter/bible/1_chronicles/25.txt")),
    ("data/pages/bible/1_chronicles/26.txt", include_str!("starter/bible/1_chronicles/26.txt")),
    ("data/pages/bible/1_chronicles/27.txt", include_str!("starter/bible/1_chronicles/27.txt")),
    ("data/pages/bible/1_chronicles/28.txt", include_str!("starter/bible/1_chronicles/28.txt")),
    ("data/pages/bible/1_chronicles/29.txt", include_str!("starter/bible/1_chronicles/29.txt")),
    ("data/pages/bible/1_chronicles/start.txt", include_str!("starter/bible/1_chronicles/start.txt")),
    ("data/pages/bible/2_chronicles/1.txt", include_str!("starter/bible/2_chronicles/1.txt")),
    ("data/pages/bible/2_chronicles/2.txt", include_str!("starter/bible/2_chronicles/2.txt")),
    ("data/pages/bible/2_chronicles/3.txt", include_str!("starter/bible/2_chronicles/3.txt")),
    ("data/pages/bible/2_chronicles/4.txt", include_str!("starter/bible/2_chronicles/4.txt")),
    ("data/pages/bible/2_chronicles/5.txt", include_str!("starter/bible/2_chronicles/5.txt")),
    ("data/pages/bible/2_chronicles/6.txt", include_str!("starter/bible/2_chronicles/6.txt")),
    ("data/pages/bible/2_chronicles/7.txt", include_str!("starter/bible/2_chronicles/7.txt")),
    ("data/pages/bible/2_chronicles/8.txt", include_str!("starter/bible/2_chronicles/8.txt")),
    ("data/pages/bible/2_chronicles/9.txt", include_str!("starter/bible/2_chronicles/9.txt")),
    ("data/pages/bible/2_chronicles/10.txt", include_str!("starter/bible/2_chronicles/10.txt")),
    ("data/pages/bible/2_chronicles/11.txt", include_str!("starter/bible/2_chronicles/11.txt")),
    ("data/pages/bible/2_chronicles/12.txt", include_str!("starter/bible/2_chronicles/12.txt")),
    ("data/pages/bible/2_chronicles/13.txt", include_str!("starter/bible/2_chronicles/13.txt")),
    ("data/pages/bible/2_chronicles/14.txt", include_str!("starter/bible/2_chronicles/14.txt")),
    ("data/pages/bible/2_chronicles/15.txt", include_str!("starter/bible/2_chronicles/15.txt")),
    ("data/pages/bible/2_chronicles/16.txt", include_str!("starter/bible/2_chronicles/16.txt")),
    ("data/pages/bible/2_chronicles/17.txt", include_str!("starter/bible/2_chronicles/17.txt")),
    ("data/pages/bible/2_chronicles/18.txt", include_str!("starter/bible/2_chronicles/18.txt")),
    ("data/pages/bible/2_chronicles/19.txt", include_str!("starter/bible/2_chronicles/19.txt")),
    ("data/pages/bible/2_chronicles/20.txt", include_str!("starter/bible/2_chronicles/20.txt")),
    ("data/pages/bible/2_chronicles/21.txt", include_str!("starter/bible/2_chronicles/21.txt")),
    ("data/pages/bible/2_chronicles/22.txt", include_str!("starter/bible/2_chronicles/22.txt")),
    ("data/pages/bible/2_chronicles/23.txt", include_str!("starter/bible/2_chronicles/23.txt")),
    ("data/pages/bible/2_chronicles/24.txt", include_str!("starter/bible/2_chronicles/24.txt")),
    ("data/pages/bible/2_chronicles/25.txt", include_str!("starter/bible/2_chronicles/25.txt")),
    ("data/pages/bible/2_chronicles/26.txt", include_str!("starter/bible/2_chronicles/26.txt")),
    ("data/pages/bible/2_chronicles/27.txt", include_str!("starter/bible/2_chronicles/27.txt")),
    ("data/pages/bible/2_chronicles/28.txt", include_str!("starter/bible/2_chronicles/28.txt")),
    ("data/pages/bible/2_chronicles/29.txt", include_str!("starter/bible/2_chronicles/29.txt")),
    ("data/pages/bible/2_chronicles/30.txt", include_str!("starter/bible/2_chronicles/30.txt")),
    ("data/pages/bible/2_chronicles/31.txt", include_str!("starter/bible/2_chronicles/31.txt")),
    ("data/pages/bible/2_chronicles/32.txt", include_str!("starter/bible/2_chronicles/32.txt")),
    ("data/pages/bible/2_chronicles/33.txt", include_str!("starter/bible/2_chronicles/33.txt")),
    ("data/pages/bible/2_chronicles/34.txt", include_str!("starter/bible/2_chronicles/34.txt")),
    ("data/pages/bible/2_chronicles/35.txt", include_str!("starter/bible/2_chronicles/35.txt")),
    ("data/pages/bible/2_chronicles/36.txt", include_str!("starter/bible/2_chronicles/36.txt")),
    ("data/pages/bible/2_chronicles/start.txt", include_str!("starter/bible/2_chronicles/start.txt")),
    ("data/pages/bible/ezra/1.txt", include_str!("starter/bible/ezra/1.txt")),
    ("data/pages/bible/ezra/2.txt", include_str!("starter/bible/ezra/2.txt")),
    ("data/pages/bible/ezra/3.txt", include_str!("starter/bible/ezra/3.txt")),
    ("data/pages/bible/ezra/4.txt", include_str!("starter/bible/ezra/4.txt")),
    ("data/pages/bible/ezra/5.txt", include_str!("starter/bible/ezra/5.txt")),
    ("data/pages/bible/ezra/6.txt", include_str!("starter/bible/ezra/6.txt")),
    ("data/pages/bible/ezra/7.txt", include_str!("starter/bible/ezra/7.txt")),
    ("data/pages/bible/ezra/8.txt", include_str!("starter/bible/ezra/8.txt")),
    ("data/pages/bible/ezra/9.txt", include_str!("starter/bible/ezra/9.txt")),
    ("data/pages/bible/ezra/10.txt", include_str!("starter/bible/ezra/10.txt")),
    ("data/pages/bible/ezra/start.txt", include_str!("starter/bible/ezra/start.txt")),
    ("data/pages/bible/nehemiah/1.txt", include_str!("starter/bible/nehemiah/1.txt")),
    ("data/pages/bible/nehemiah/2.txt", include_str!("starter/bible/nehemiah/2.txt")),
    ("data/pages/bible/nehemiah/3.txt", include_str!("starter/bible/nehemiah/3.txt")),
    ("data/pages/bible/nehemiah/4.txt", include_str!("starter/bible/nehemiah/4.txt")),
    ("data/pages/bible/nehemiah/5.txt", include_str!("starter/bible/nehemiah/5.txt")),
    ("data/pages/bible/nehemiah/6.txt", include_str!("starter/bible/nehemiah/6.txt")),
    ("data/pages/bible/nehemiah/7.txt", include_str!("starter/bible/nehemiah/7.txt")),
    ("data/pages/bible/nehemiah/8.txt", include_str!("starter/bible/nehemiah/8.txt")),
    ("data/pages/bible/nehemiah/9.txt", include_str!("starter/bible/nehemiah/9.txt")),
    ("data/pages/bible/nehemiah/10.txt", include_str!("starter/bible/nehemiah/10.txt")),
    ("data/pages/bible/nehemiah/11.txt", include_str!("starter/bible/nehemiah/11.txt")),
    ("data/pages/bible/nehemiah/12.txt", include_str!("starter/bible/nehemiah/12.txt")),
    ("data/pages/bible/nehemiah/13.txt", include_str!("starter/bible/nehemiah/13.txt")),
    ("data/pages/bible/nehemiah/start.txt", include_str!("starter/bible/nehemiah/start.txt")),
    ("data/pages/bible/esther/1.txt", include_str!("starter/bible/esther/1.txt")),
    ("data/pages/bible/esther/2.txt", include_str!("starter/bible/esther/2.txt")),
    ("data/pages/bible/esther/3.txt", include_str!("starter/bible/esther/3.txt")),
    ("data/pages/bible/esther/4.txt", include_str!("starter/bible/esther/4.txt")),
    ("data/pages/bible/esther/5.txt", include_str!("starter/bible/esther/5.txt")),
    ("data/pages/bible/esther/6.txt", include_str!("starter/bible/esther/6.txt")),
    ("data/pages/bible/esther/7.txt", include_str!("starter/bible/esther/7.txt")),
    ("data/pages/bible/esther/8.txt", include_str!("starter/bible/esther/8.txt")),
    ("data/pages/bible/esther/9.txt", include_str!("starter/bible/esther/9.txt")),
    ("data/pages/bible/esther/10.txt", include_str!("starter/bible/esther/10.txt")),
    ("data/pages/bible/esther/start.txt", include_str!("starter/bible/esther/start.txt")),
    ("data/pages/bible/job/1.txt", include_str!("starter/bible/job/1.txt")),
    ("data/pages/bible/job/2.txt", include_str!("starter/bible/job/2.txt")),
    ("data/pages/bible/job/3.txt", include_str!("starter/bible/job/3.txt")),
    ("data/pages/bible/job/4.txt", include_str!("starter/bible/job/4.txt")),
    ("data/pages/bible/job/5.txt", include_str!("starter/bible/job/5.txt")),
    ("data/pages/bible/job/6.txt", include_str!("starter/bible/job/6.txt")),
    ("data/pages/bible/job/7.txt", include_str!("starter/bible/job/7.txt")),
    ("data/pages/bible/job/8.txt", include_str!("starter/bible/job/8.txt")),
    ("data/pages/bible/job/9.txt", include_str!("starter/bible/job/9.txt")),
    ("data/pages/bible/job/10.txt", include_str!("starter/bible/job/10.txt")),
    ("data/pages/bible/job/11.txt", include_str!("starter/bible/job/11.txt")),
    ("data/pages/bible/job/12.txt", include_str!("starter/bible/job/12.txt")),
    ("data/pages/bible/job/13.txt", include_str!("starter/bible/job/13.txt")),
    ("data/pages/bible/job/14.txt", include_str!("starter/bible/job/14.txt")),
    ("data/pages/bible/job/15.txt", include_str!("starter/bible/job/15.txt")),
    ("data/pages/bible/job/16.txt", include_str!("starter/bible/job/16.txt")),
    ("data/pages/bible/job/17.txt", include_str!("starter/bible/job/17.txt")),
    ("data/pages/bible/job/18.txt", include_str!("starter/bible/job/18.txt")),
    ("data/pages/bible/job/19.txt", include_str!("starter/bible/job/19.txt")),
    ("data/pages/bible/job/20.txt", include_str!("starter/bible/job/20.txt")),
    ("data/pages/bible/job/21.txt", include_str!("starter/bible/job/21.txt")),
    ("data/pages/bible/job/22.txt", include_str!("starter/bible/job/22.txt")),
    ("data/pages/bible/job/23.txt", include_str!("starter/bible/job/23.txt")),
    ("data/pages/bible/job/24.txt", include_str!("starter/bible/job/24.txt")),
    ("data/pages/bible/job/25.txt", include_str!("starter/bible/job/25.txt")),
    ("data/pages/bible/job/26.txt", include_str!("starter/bible/job/26.txt")),
    ("data/pages/bible/job/27.txt", include_str!("starter/bible/job/27.txt")),
    ("data/pages/bible/job/28.txt", include_str!("starter/bible/job/28.txt")),
    ("data/pages/bible/job/29.txt", include_str!("starter/bible/job/29.txt")),
    ("data/pages/bible/job/30.txt", include_str!("starter/bible/job/30.txt")),
    ("data/pages/bible/job/31.txt", include_str!("starter/bible/job/31.txt")),
    ("data/pages/bible/job/32.txt", include_str!("starter/bible/job/32.txt")),
    ("data/pages/bible/job/33.txt", include_str!("starter/bible/job/33.txt")),
    ("data/pages/bible/job/34.txt", include_str!("starter/bible/job/34.txt")),
    ("data/pages/bible/job/35.txt", include_str!("starter/bible/job/35.txt")),
    ("data/pages/bible/job/36.txt", include_str!("starter/bible/job/36.txt")),
    ("data/pages/bible/job/37.txt", include_str!("starter/bible/job/37.txt")),
    ("data/pages/bible/job/38.txt", include_str!("starter/bible/job/38.txt")),
    ("data/pages/bible/job/39.txt", include_str!("starter/bible/job/39.txt")),
    ("data/pages/bible/job/40.txt", include_str!("starter/bible/job/40.txt")),
    ("data/pages/bible/job/41.txt", include_str!("starter/bible/job/41.txt")),
    ("data/pages/bible/job/42.txt", include_str!("starter/bible/job/42.txt")),
    ("data/pages/bible/job/start.txt", include_str!("starter/bible/job/start.txt")),
    ("data/pages/bible/psalms/1.txt", include_str!("starter/bible/psalms/1.txt")),
    ("data/pages/bible/psalms/2.txt", include_str!("starter/bible/psalms/2.txt")),
    ("data/pages/bible/psalms/3.txt", include_str!("starter/bible/psalms/3.txt")),
    ("data/pages/bible/psalms/4.txt", include_str!("starter/bible/psalms/4.txt")),
    ("data/pages/bible/psalms/5.txt", include_str!("starter/bible/psalms/5.txt")),
    ("data/pages/bible/psalms/6.txt", include_str!("starter/bible/psalms/6.txt")),
    ("data/pages/bible/psalms/7.txt", include_str!("starter/bible/psalms/7.txt")),
    ("data/pages/bible/psalms/8.txt", include_str!("starter/bible/psalms/8.txt")),
    ("data/pages/bible/psalms/9.txt", include_str!("starter/bible/psalms/9.txt")),
    ("data/pages/bible/psalms/10.txt", include_str!("starter/bible/psalms/10.txt")),
    ("data/pages/bible/psalms/11.txt", include_str!("starter/bible/psalms/11.txt")),
    ("data/pages/bible/psalms/12.txt", include_str!("starter/bible/psalms/12.txt")),
    ("data/pages/bible/psalms/13.txt", include_str!("starter/bible/psalms/13.txt")),
    ("data/pages/bible/psalms/14.txt", include_str!("starter/bible/psalms/14.txt")),
    ("data/pages/bible/psalms/15.txt", include_str!("starter/bible/psalms/15.txt")),
    ("data/pages/bible/psalms/16.txt", include_str!("starter/bible/psalms/16.txt")),
    ("data/pages/bible/psalms/17.txt", include_str!("starter/bible/psalms/17.txt")),
    ("data/pages/bible/psalms/18.txt", include_str!("starter/bible/psalms/18.txt")),
    ("data/pages/bible/psalms/19.txt", include_str!("starter/bible/psalms/19.txt")),
    ("data/pages/bible/psalms/20.txt", include_str!("starter/bible/psalms/20.txt")),
    ("data/pages/bible/psalms/21.txt", include_str!("starter/bible/psalms/21.txt")),
    ("data/pages/bible/psalms/22.txt", include_str!("starter/bible/psalms/22.txt")),
    ("data/pages/bible/psalms/23.txt", include_str!("starter/bible/psalms/23.txt")),
    ("data/pages/bible/psalms/24.txt", include_str!("starter/bible/psalms/24.txt")),
    ("data/pages/bible/psalms/25.txt", include_str!("starter/bible/psalms/25.txt")),
    ("data/pages/bible/psalms/26.txt", include_str!("starter/bible/psalms/26.txt")),
    ("data/pages/bible/psalms/27.txt", include_str!("starter/bible/psalms/27.txt")),
    ("data/pages/bible/psalms/28.txt", include_str!("starter/bible/psalms/28.txt")),
    ("data/pages/bible/psalms/29.txt", include_str!("starter/bible/psalms/29.txt")),
    ("data/pages/bible/psalms/30.txt", include_str!("starter/bible/psalms/30.txt")),
    ("data/pages/bible/psalms/31.txt", include_str!("starter/bible/psalms/31.txt")),
    ("data/pages/bible/psalms/32.txt", include_str!("starter/bible/psalms/32.txt")),
    ("data/pages/bible/psalms/33.txt", include_str!("starter/bible/psalms/33.txt")),
    ("data/pages/bible/psalms/34.txt", include_str!("starter/bible/psalms/34.txt")),
    ("data/pages/bible/psalms/35.txt", include_str!("starter/bible/psalms/35.txt")),
    ("data/pages/bible/psalms/36.txt", include_str!("starter/bible/psalms/36.txt")),
    ("data/pages/bible/psalms/37.txt", include_str!("starter/bible/psalms/37.txt")),
    ("data/pages/bible/psalms/38.txt", include_str!("starter/bible/psalms/38.txt")),
    ("data/pages/bible/psalms/39.txt", include_str!("starter/bible/psalms/39.txt")),
    ("data/pages/bible/psalms/40.txt", include_str!("starter/bible/psalms/40.txt")),
    ("data/pages/bible/psalms/41.txt", include_str!("starter/bible/psalms/41.txt")),
    ("data/pages/bible/psalms/42.txt", include_str!("starter/bible/psalms/42.txt")),
    ("data/pages/bible/psalms/43.txt", include_str!("starter/bible/psalms/43.txt")),
    ("data/pages/bible/psalms/44.txt", include_str!("starter/bible/psalms/44.txt")),
    ("data/pages/bible/psalms/45.txt", include_str!("starter/bible/psalms/45.txt")),
    ("data/pages/bible/psalms/46.txt", include_str!("starter/bible/psalms/46.txt")),
    ("data/pages/bible/psalms/47.txt", include_str!("starter/bible/psalms/47.txt")),
    ("data/pages/bible/psalms/48.txt", include_str!("starter/bible/psalms/48.txt")),
    ("data/pages/bible/psalms/49.txt", include_str!("starter/bible/psalms/49.txt")),
    ("data/pages/bible/psalms/50.txt", include_str!("starter/bible/psalms/50.txt")),
    ("data/pages/bible/psalms/51.txt", include_str!("starter/bible/psalms/51.txt")),
    ("data/pages/bible/psalms/52.txt", include_str!("starter/bible/psalms/52.txt")),
    ("data/pages/bible/psalms/53.txt", include_str!("starter/bible/psalms/53.txt")),
    ("data/pages/bible/psalms/54.txt", include_str!("starter/bible/psalms/54.txt")),
    ("data/pages/bible/psalms/55.txt", include_str!("starter/bible/psalms/55.txt")),
    ("data/pages/bible/psalms/56.txt", include_str!("starter/bible/psalms/56.txt")),
    ("data/pages/bible/psalms/57.txt", include_str!("starter/bible/psalms/57.txt")),
    ("data/pages/bible/psalms/58.txt", include_str!("starter/bible/psalms/58.txt")),
    ("data/pages/bible/psalms/59.txt", include_str!("starter/bible/psalms/59.txt")),
    ("data/pages/bible/psalms/60.txt", include_str!("starter/bible/psalms/60.txt")),
    ("data/pages/bible/psalms/61.txt", include_str!("starter/bible/psalms/61.txt")),
    ("data/pages/bible/psalms/62.txt", include_str!("starter/bible/psalms/62.txt")),
    ("data/pages/bible/psalms/63.txt", include_str!("starter/bible/psalms/63.txt")),
    ("data/pages/bible/psalms/64.txt", include_str!("starter/bible/psalms/64.txt")),
    ("data/pages/bible/psalms/65.txt", include_str!("starter/bible/psalms/65.txt")),
    ("data/pages/bible/psalms/66.txt", include_str!("starter/bible/psalms/66.txt")),
    ("data/pages/bible/psalms/67.txt", include_str!("starter/bible/psalms/67.txt")),
    ("data/pages/bible/psalms/68.txt", include_str!("starter/bible/psalms/68.txt")),
    ("data/pages/bible/psalms/69.txt", include_str!("starter/bible/psalms/69.txt")),
    ("data/pages/bible/psalms/70.txt", include_str!("starter/bible/psalms/70.txt")),
    ("data/pages/bible/psalms/71.txt", include_str!("starter/bible/psalms/71.txt")),
    ("data/pages/bible/psalms/72.txt", include_str!("starter/bible/psalms/72.txt")),
    ("data/pages/bible/psalms/73.txt", include_str!("starter/bible/psalms/73.txt")),
    ("data/pages/bible/psalms/74.txt", include_str!("starter/bible/psalms/74.txt")),
    ("data/pages/bible/psalms/75.txt", include_str!("starter/bible/psalms/75.txt")),
    ("data/pages/bible/psalms/76.txt", include_str!("starter/bible/psalms/76.txt")),
    ("data/pages/bible/psalms/77.txt", include_str!("starter/bible/psalms/77.txt")),
    ("data/pages/bible/psalms/78.txt", include_str!("starter/bible/psalms/78.txt")),
    ("data/pages/bible/psalms/79.txt", include_str!("starter/bible/psalms/79.txt")),
    ("data/pages/bible/psalms/80.txt", include_str!("starter/bible/psalms/80.txt")),
    ("data/pages/bible/psalms/81.txt", include_str!("starter/bible/psalms/81.txt")),
    ("data/pages/bible/psalms/82.txt", include_str!("starter/bible/psalms/82.txt")),
    ("data/pages/bible/psalms/83.txt", include_str!("starter/bible/psalms/83.txt")),
    ("data/pages/bible/psalms/84.txt", include_str!("starter/bible/psalms/84.txt")),
    ("data/pages/bible/psalms/85.txt", include_str!("starter/bible/psalms/85.txt")),
    ("data/pages/bible/psalms/86.txt", include_str!("starter/bible/psalms/86.txt")),
    ("data/pages/bible/psalms/87.txt", include_str!("starter/bible/psalms/87.txt")),
    ("data/pages/bible/psalms/88.txt", include_str!("starter/bible/psalms/88.txt")),
    ("data/pages/bible/psalms/89.txt", include_str!("starter/bible/psalms/89.txt")),
    ("data/pages/bible/psalms/90.txt", include_str!("starter/bible/psalms/90.txt")),
    ("data/pages/bible/psalms/91.txt", include_str!("starter/bible/psalms/91.txt")),
    ("data/pages/bible/psalms/92.txt", include_str!("starter/bible/psalms/92.txt")),
    ("data/pages/bible/psalms/93.txt", include_str!("starter/bible/psalms/93.txt")),
    ("data/pages/bible/psalms/94.txt", include_str!("starter/bible/psalms/94.txt")),
    ("data/pages/bible/psalms/95.txt", include_str!("starter/bible/psalms/95.txt")),
    ("data/pages/bible/psalms/96.txt", include_str!("starter/bible/psalms/96.txt")),
    ("data/pages/bible/psalms/97.txt", include_str!("starter/bible/psalms/97.txt")),
    ("data/pages/bible/psalms/98.txt", include_str!("starter/bible/psalms/98.txt")),
    ("data/pages/bible/psalms/99.txt", include_str!("starter/bible/psalms/99.txt")),
    ("data/pages/bible/psalms/100.txt", include_str!("starter/bible/psalms/100.txt")),
    ("data/pages/bible/psalms/101.txt", include_str!("starter/bible/psalms/101.txt")),
    ("data/pages/bible/psalms/102.txt", include_str!("starter/bible/psalms/102.txt")),
    ("data/pages/bible/psalms/103.txt", include_str!("starter/bible/psalms/103.txt")),
    ("data/pages/bible/psalms/104.txt", include_str!("starter/bible/psalms/104.txt")),
    ("data/pages/bible/psalms/105.txt", include_str!("starter/bible/psalms/105.txt")),
    ("data/pages/bible/psalms/106.txt", include_str!("starter/bible/psalms/106.txt")),
    ("data/pages/bible/psalms/107.txt", include_str!("starter/bible/psalms/107.txt")),
    ("data/pages/bible/psalms/108.txt", include_str!("starter/bible/psalms/108.txt")),
    ("data/pages/bible/psalms/109.txt", include_str!("starter/bible/psalms/109.txt")),
    ("data/pages/bible/psalms/110.txt", include_str!("starter/bible/psalms/110.txt")),
    ("data/pages/bible/psalms/111.txt", include_str!("starter/bible/psalms/111.txt")),
    ("data/pages/bible/psalms/112.txt", include_str!("starter/bible/psalms/112.txt")),
    ("data/pages/bible/psalms/113.txt", include_str!("starter/bible/psalms/113.txt")),
    ("data/pages/bible/psalms/114.txt", include_str!("starter/bible/psalms/114.txt")),
    ("data/pages/bible/psalms/115.txt", include_str!("starter/bible/psalms/115.txt")),
    ("data/pages/bible/psalms/116.txt", include_str!("starter/bible/psalms/116.txt")),
    ("data/pages/bible/psalms/117.txt", include_str!("starter/bible/psalms/117.txt")),
    ("data/pages/bible/psalms/118.txt", include_str!("starter/bible/psalms/118.txt")),
    ("data/pages/bible/psalms/119.txt", include_str!("starter/bible/psalms/119.txt")),
    ("data/pages/bible/psalms/120.txt", include_str!("starter/bible/psalms/120.txt")),
    ("data/pages/bible/psalms/121.txt", include_str!("starter/bible/psalms/121.txt")),
    ("data/pages/bible/psalms/122.txt", include_str!("starter/bible/psalms/122.txt")),
    ("data/pages/bible/psalms/123.txt", include_str!("starter/bible/psalms/123.txt")),
    ("data/pages/bible/psalms/124.txt", include_str!("starter/bible/psalms/124.txt")),
    ("data/pages/bible/psalms/125.txt", include_str!("starter/bible/psalms/125.txt")),
    ("data/pages/bible/psalms/126.txt", include_str!("starter/bible/psalms/126.txt")),
    ("data/pages/bible/psalms/127.txt", include_str!("starter/bible/psalms/127.txt")),
    ("data/pages/bible/psalms/128.txt", include_str!("starter/bible/psalms/128.txt")),
    ("data/pages/bible/psalms/129.txt", include_str!("starter/bible/psalms/129.txt")),
    ("data/pages/bible/psalms/130.txt", include_str!("starter/bible/psalms/130.txt")),
    ("data/pages/bible/psalms/131.txt", include_str!("starter/bible/psalms/131.txt")),
    ("data/pages/bible/psalms/132.txt", include_str!("starter/bible/psalms/132.txt")),
    ("data/pages/bible/psalms/133.txt", include_str!("starter/bible/psalms/133.txt")),
    ("data/pages/bible/psalms/134.txt", include_str!("starter/bible/psalms/134.txt")),
    ("data/pages/bible/psalms/135.txt", include_str!("starter/bible/psalms/135.txt")),
    ("data/pages/bible/psalms/136.txt", include_str!("starter/bible/psalms/136.txt")),
    ("data/pages/bible/psalms/137.txt", include_str!("starter/bible/psalms/137.txt")),
    ("data/pages/bible/psalms/138.txt", include_str!("starter/bible/psalms/138.txt")),
    ("data/pages/bible/psalms/139.txt", include_str!("starter/bible/psalms/139.txt")),
    ("data/pages/bible/psalms/140.txt", include_str!("starter/bible/psalms/140.txt")),
    ("data/pages/bible/psalms/141.txt", include_str!("starter/bible/psalms/141.txt")),
    ("data/pages/bible/psalms/142.txt", include_str!("starter/bible/psalms/142.txt")),
    ("data/pages/bible/psalms/143.txt", include_str!("starter/bible/psalms/143.txt")),
    ("data/pages/bible/psalms/144.txt", include_str!("starter/bible/psalms/144.txt")),
    ("data/pages/bible/psalms/145.txt", include_str!("starter/bible/psalms/145.txt")),
    ("data/pages/bible/psalms/146.txt", include_str!("starter/bible/psalms/146.txt")),
    ("data/pages/bible/psalms/147.txt", include_str!("starter/bible/psalms/147.txt")),
    ("data/pages/bible/psalms/148.txt", include_str!("starter/bible/psalms/148.txt")),
    ("data/pages/bible/psalms/149.txt", include_str!("starter/bible/psalms/149.txt")),
    ("data/pages/bible/psalms/150.txt", include_str!("starter/bible/psalms/150.txt")),
    ("data/pages/bible/psalms/start.txt", include_str!("starter/bible/psalms/start.txt")),
    ("data/pages/bible/proverbs/1.txt", include_str!("starter/bible/proverbs/1.txt")),
    ("data/pages/bible/proverbs/2.txt", include_str!("starter/bible/proverbs/2.txt")),
    ("data/pages/bible/proverbs/3.txt", include_str!("starter/bible/proverbs/3.txt")),
    ("data/pages/bible/proverbs/4.txt", include_str!("starter/bible/proverbs/4.txt")),
    ("data/pages/bible/proverbs/5.txt", include_str!("starter/bible/proverbs/5.txt")),
    ("data/pages/bible/proverbs/6.txt", include_str!("starter/bible/proverbs/6.txt")),
    ("data/pages/bible/proverbs/7.txt", include_str!("starter/bible/proverbs/7.txt")),
    ("data/pages/bible/proverbs/8.txt", include_str!("starter/bible/proverbs/8.txt")),
    ("data/pages/bible/proverbs/9.txt", include_str!("starter/bible/proverbs/9.txt")),
    ("data/pages/bible/proverbs/10.txt", include_str!("starter/bible/proverbs/10.txt")),
    ("data/pages/bible/proverbs/11.txt", include_str!("starter/bible/proverbs/11.txt")),
    ("data/pages/bible/proverbs/12.txt", include_str!("starter/bible/proverbs/12.txt")),
    ("data/pages/bible/proverbs/13.txt", include_str!("starter/bible/proverbs/13.txt")),
    ("data/pages/bible/proverbs/14.txt", include_str!("starter/bible/proverbs/14.txt")),
    ("data/pages/bible/proverbs/15.txt", include_str!("starter/bible/proverbs/15.txt")),
    ("data/pages/bible/proverbs/16.txt", include_str!("starter/bible/proverbs/16.txt")),
    ("data/pages/bible/proverbs/17.txt", include_str!("starter/bible/proverbs/17.txt")),
    ("data/pages/bible/proverbs/18.txt", include_str!("starter/bible/proverbs/18.txt")),
    ("data/pages/bible/proverbs/19.txt", include_str!("starter/bible/proverbs/19.txt")),
    ("data/pages/bible/proverbs/20.txt", include_str!("starter/bible/proverbs/20.txt")),
    ("data/pages/bible/proverbs/21.txt", include_str!("starter/bible/proverbs/21.txt")),
    ("data/pages/bible/proverbs/22.txt", include_str!("starter/bible/proverbs/22.txt")),
    ("data/pages/bible/proverbs/23.txt", include_str!("starter/bible/proverbs/23.txt")),
    ("data/pages/bible/proverbs/24.txt", include_str!("starter/bible/proverbs/24.txt")),
    ("data/pages/bible/proverbs/25.txt", include_str!("starter/bible/proverbs/25.txt")),
    ("data/pages/bible/proverbs/26.txt", include_str!("starter/bible/proverbs/26.txt")),
    ("data/pages/bible/proverbs/27.txt", include_str!("starter/bible/proverbs/27.txt")),
    ("data/pages/bible/proverbs/28.txt", include_str!("starter/bible/proverbs/28.txt")),
    ("data/pages/bible/proverbs/29.txt", include_str!("starter/bible/proverbs/29.txt")),
    ("data/pages/bible/proverbs/30.txt", include_str!("starter/bible/proverbs/30.txt")),
    ("data/pages/bible/proverbs/31.txt", include_str!("starter/bible/proverbs/31.txt")),
    ("data/pages/bible/proverbs/start.txt", include_str!("starter/bible/proverbs/start.txt")),
    ("data/pages/bible/ecclesiastes/1.txt", include_str!("starter/bible/ecclesiastes/1.txt")),
    ("data/pages/bible/ecclesiastes/2.txt", include_str!("starter/bible/ecclesiastes/2.txt")),
    ("data/pages/bible/ecclesiastes/3.txt", include_str!("starter/bible/ecclesiastes/3.txt")),
    ("data/pages/bible/ecclesiastes/4.txt", include_str!("starter/bible/ecclesiastes/4.txt")),
    ("data/pages/bible/ecclesiastes/5.txt", include_str!("starter/bible/ecclesiastes/5.txt")),
    ("data/pages/bible/ecclesiastes/6.txt", include_str!("starter/bible/ecclesiastes/6.txt")),
    ("data/pages/bible/ecclesiastes/7.txt", include_str!("starter/bible/ecclesiastes/7.txt")),
    ("data/pages/bible/ecclesiastes/8.txt", include_str!("starter/bible/ecclesiastes/8.txt")),
    ("data/pages/bible/ecclesiastes/9.txt", include_str!("starter/bible/ecclesiastes/9.txt")),
    ("data/pages/bible/ecclesiastes/10.txt", include_str!("starter/bible/ecclesiastes/10.txt")),
    ("data/pages/bible/ecclesiastes/11.txt", include_str!("starter/bible/ecclesiastes/11.txt")),
    ("data/pages/bible/ecclesiastes/12.txt", include_str!("starter/bible/ecclesiastes/12.txt")),
    ("data/pages/bible/ecclesiastes/start.txt", include_str!("starter/bible/ecclesiastes/start.txt")),
    ("data/pages/bible/song_of_solomon/1.txt", include_str!("starter/bible/song_of_solomon/1.txt")),
    ("data/pages/bible/song_of_solomon/2.txt", include_str!("starter/bible/song_of_solomon/2.txt")),
    ("data/pages/bible/song_of_solomon/3.txt", include_str!("starter/bible/song_of_solomon/3.txt")),
    ("data/pages/bible/song_of_solomon/4.txt", include_str!("starter/bible/song_of_solomon/4.txt")),
    ("data/pages/bible/song_of_solomon/5.txt", include_str!("starter/bible/song_of_solomon/5.txt")),
    ("data/pages/bible/song_of_solomon/6.txt", include_str!("starter/bible/song_of_solomon/6.txt")),
    ("data/pages/bible/song_of_solomon/7.txt", include_str!("starter/bible/song_of_solomon/7.txt")),
    ("data/pages/bible/song_of_solomon/8.txt", include_str!("starter/bible/song_of_solomon/8.txt")),
    ("data/pages/bible/song_of_solomon/start.txt", include_str!("starter/bible/song_of_solomon/start.txt")),
    ("data/pages/bible/isaiah/1.txt", include_str!("starter/bible/isaiah/1.txt")),
    ("data/pages/bible/isaiah/2.txt", include_str!("starter/bible/isaiah/2.txt")),
    ("data/pages/bible/isaiah/3.txt", include_str!("starter/bible/isaiah/3.txt")),
    ("data/pages/bible/isaiah/4.txt", include_str!("starter/bible/isaiah/4.txt")),
    ("data/pages/bible/isaiah/5.txt", include_str!("starter/bible/isaiah/5.txt")),
    ("data/pages/bible/isaiah/6.txt", include_str!("starter/bible/isaiah/6.txt")),
    ("data/pages/bible/isaiah/7.txt", include_str!("starter/bible/isaiah/7.txt")),
    ("data/pages/bible/isaiah/8.txt", include_str!("starter/bible/isaiah/8.txt")),
    ("data/pages/bible/isaiah/9.txt", include_str!("starter/bible/isaiah/9.txt")),
    ("data/pages/bible/isaiah/10.txt", include_str!("starter/bible/isaiah/10.txt")),
    ("data/pages/bible/isaiah/11.txt", include_str!("starter/bible/isaiah/11.txt")),
    ("data/pages/bible/isaiah/12.txt", include_str!("starter/bible/isaiah/12.txt")),
    ("data/pages/bible/isaiah/13.txt", include_str!("starter/bible/isaiah/13.txt")),
    ("data/pages/bible/isaiah/14.txt", include_str!("starter/bible/isaiah/14.txt")),
    ("data/pages/bible/isaiah/15.txt", include_str!("starter/bible/isaiah/15.txt")),
    ("data/pages/bible/isaiah/16.txt", include_str!("starter/bible/isaiah/16.txt")),
    ("data/pages/bible/isaiah/17.txt", include_str!("starter/bible/isaiah/17.txt")),
    ("data/pages/bible/isaiah/18.txt", include_str!("starter/bible/isaiah/18.txt")),
    ("data/pages/bible/isaiah/19.txt", include_str!("starter/bible/isaiah/19.txt")),
    ("data/pages/bible/isaiah/20.txt", include_str!("starter/bible/isaiah/20.txt")),
    ("data/pages/bible/isaiah/21.txt", include_str!("starter/bible/isaiah/21.txt")),
    ("data/pages/bible/isaiah/22.txt", include_str!("starter/bible/isaiah/22.txt")),
    ("data/pages/bible/isaiah/23.txt", include_str!("starter/bible/isaiah/23.txt")),
    ("data/pages/bible/isaiah/24.txt", include_str!("starter/bible/isaiah/24.txt")),
    ("data/pages/bible/isaiah/25.txt", include_str!("starter/bible/isaiah/25.txt")),
    ("data/pages/bible/isaiah/26.txt", include_str!("starter/bible/isaiah/26.txt")),
    ("data/pages/bible/isaiah/27.txt", include_str!("starter/bible/isaiah/27.txt")),
    ("data/pages/bible/isaiah/28.txt", include_str!("starter/bible/isaiah/28.txt")),
    ("data/pages/bible/isaiah/29.txt", include_str!("starter/bible/isaiah/29.txt")),
    ("data/pages/bible/isaiah/30.txt", include_str!("starter/bible/isaiah/30.txt")),
    ("data/pages/bible/isaiah/31.txt", include_str!("starter/bible/isaiah/31.txt")),
    ("data/pages/bible/isaiah/32.txt", include_str!("starter/bible/isaiah/32.txt")),
    ("data/pages/bible/isaiah/33.txt", include_str!("starter/bible/isaiah/33.txt")),
    ("data/pages/bible/isaiah/34.txt", include_str!("starter/bible/isaiah/34.txt")),
    ("data/pages/bible/isaiah/35.txt", include_str!("starter/bible/isaiah/35.txt")),
    ("data/pages/bible/isaiah/36.txt", include_str!("starter/bible/isaiah/36.txt")),
    ("data/pages/bible/isaiah/37.txt", include_str!("starter/bible/isaiah/37.txt")),
    ("data/pages/bible/isaiah/38.txt", include_str!("starter/bible/isaiah/38.txt")),
    ("data/pages/bible/isaiah/39.txt", include_str!("starter/bible/isaiah/39.txt")),
    ("data/pages/bible/isaiah/40.txt", include_str!("starter/bible/isaiah/40.txt")),
    ("data/pages/bible/isaiah/41.txt", include_str!("starter/bible/isaiah/41.txt")),
    ("data/pages/bible/isaiah/42.txt", include_str!("starter/bible/isaiah/42.txt")),
    ("data/pages/bible/isaiah/43.txt", include_str!("starter/bible/isaiah/43.txt")),
    ("data/pages/bible/isaiah/44.txt", include_str!("starter/bible/isaiah/44.txt")),
    ("data/pages/bible/isaiah/45.txt", include_str!("starter/bible/isaiah/45.txt")),
    ("data/pages/bible/isaiah/46.txt", include_str!("starter/bible/isaiah/46.txt")),
    ("data/pages/bible/isaiah/47.txt", include_str!("starter/bible/isaiah/47.txt")),
    ("data/pages/bible/isaiah/48.txt", include_str!("starter/bible/isaiah/48.txt")),
    ("data/pages/bible/isaiah/49.txt", include_str!("starter/bible/isaiah/49.txt")),
    ("data/pages/bible/isaiah/50.txt", include_str!("starter/bible/isaiah/50.txt")),
    ("data/pages/bible/isaiah/51.txt", include_str!("starter/bible/isaiah/51.txt")),
    ("data/pages/bible/isaiah/52.txt", include_str!("starter/bible/isaiah/52.txt")),
    ("data/pages/bible/isaiah/53.txt", include_str!("starter/bible/isaiah/53.txt")),
    ("data/pages/bible/isaiah/54.txt", include_str!("starter/bible/isaiah/54.txt")),
    ("data/pages/bible/isaiah/55.txt", include_str!("starter/bible/isaiah/55.txt")),
    ("data/pages/bible/isaiah/56.txt", include_str!("starter/bible/isaiah/56.txt")),
    ("data/pages/bible/isaiah/57.txt", include_str!("starter/bible/isaiah/57.txt")),
    ("data/pages/bible/isaiah/58.txt", include_str!("starter/bible/isaiah/58.txt")),
    ("data/pages/bible/isaiah/59.txt", include_str!("starter/bible/isaiah/59.txt")),
    ("data/pages/bible/isaiah/60.txt", include_str!("starter/bible/isaiah/60.txt")),
    ("data/pages/bible/isaiah/61.txt", include_str!("starter/bible/isaiah/61.txt")),
    ("data/pages/bible/isaiah/62.txt", include_str!("starter/bible/isaiah/62.txt")),
    ("data/pages/bible/isaiah/63.txt", include_str!("starter/bible/isaiah/63.txt")),
    ("data/pages/bible/isaiah/64.txt", include_str!("starter/bible/isaiah/64.txt")),
    ("data/pages/bible/isaiah/65.txt", include_str!("starter/bible/isaiah/65.txt")),
    ("data/pages/bible/isaiah/66.txt", include_str!("starter/bible/isaiah/66.txt")),
    ("data/pages/bible/isaiah/start.txt", include_str!("starter/bible/isaiah/start.txt")),
    ("data/pages/bible/jeremiah/1.txt", include_str!("starter/bible/jeremiah/1.txt")),
    ("data/pages/bible/jeremiah/2.txt", include_str!("starter/bible/jeremiah/2.txt")),
    ("data/pages/bible/jeremiah/3.txt", include_str!("starter/bible/jeremiah/3.txt")),
    ("data/pages/bible/jeremiah/4.txt", include_str!("starter/bible/jeremiah/4.txt")),
    ("data/pages/bible/jeremiah/5.txt", include_str!("starter/bible/jeremiah/5.txt")),
    ("data/pages/bible/jeremiah/6.txt", include_str!("starter/bible/jeremiah/6.txt")),
    ("data/pages/bible/jeremiah/7.txt", include_str!("starter/bible/jeremiah/7.txt")),
    ("data/pages/bible/jeremiah/8.txt", include_str!("starter/bible/jeremiah/8.txt")),
    ("data/pages/bible/jeremiah/9.txt", include_str!("starter/bible/jeremiah/9.txt")),
    ("data/pages/bible/jeremiah/10.txt", include_str!("starter/bible/jeremiah/10.txt")),
    ("data/pages/bible/jeremiah/11.txt", include_str!("starter/bible/jeremiah/11.txt")),
    ("data/pages/bible/jeremiah/12.txt", include_str!("starter/bible/jeremiah/12.txt")),
    ("data/pages/bible/jeremiah/13.txt", include_str!("starter/bible/jeremiah/13.txt")),
    ("data/pages/bible/jeremiah/14.txt", include_str!("starter/bible/jeremiah/14.txt")),
    ("data/pages/bible/jeremiah/15.txt", include_str!("starter/bible/jeremiah/15.txt")),
    ("data/pages/bible/jeremiah/16.txt", include_str!("starter/bible/jeremiah/16.txt")),
    ("data/pages/bible/jeremiah/17.txt", include_str!("starter/bible/jeremiah/17.txt")),
    ("data/pages/bible/jeremiah/18.txt", include_str!("starter/bible/jeremiah/18.txt")),
    ("data/pages/bible/jeremiah/19.txt", include_str!("starter/bible/jeremiah/19.txt")),
    ("data/pages/bible/jeremiah/20.txt", include_str!("starter/bible/jeremiah/20.txt")),
    ("data/pages/bible/jeremiah/21.txt", include_str!("starter/bible/jeremiah/21.txt")),
    ("data/pages/bible/jeremiah/22.txt", include_str!("starter/bible/jeremiah/22.txt")),
    ("data/pages/bible/jeremiah/23.txt", include_str!("starter/bible/jeremiah/23.txt")),
    ("data/pages/bible/jeremiah/24.txt", include_str!("starter/bible/jeremiah/24.txt")),
    ("data/pages/bible/jeremiah/25.txt", include_str!("starter/bible/jeremiah/25.txt")),
    ("data/pages/bible/jeremiah/26.txt", include_str!("starter/bible/jeremiah/26.txt")),
    ("data/pages/bible/jeremiah/27.txt", include_str!("starter/bible/jeremiah/27.txt")),
    ("data/pages/bible/jeremiah/28.txt", include_str!("starter/bible/jeremiah/28.txt")),
    ("data/pages/bible/jeremiah/29.txt", include_str!("starter/bible/jeremiah/29.txt")),
    ("data/pages/bible/jeremiah/30.txt", include_str!("starter/bible/jeremiah/30.txt")),
    ("data/pages/bible/jeremiah/31.txt", include_str!("starter/bible/jeremiah/31.txt")),
    ("data/pages/bible/jeremiah/32.txt", include_str!("starter/bible/jeremiah/32.txt")),
    ("data/pages/bible/jeremiah/33.txt", include_str!("starter/bible/jeremiah/33.txt")),
    ("data/pages/bible/jeremiah/34.txt", include_str!("starter/bible/jeremiah/34.txt")),
    ("data/pages/bible/jeremiah/35.txt", include_str!("starter/bible/jeremiah/35.txt")),
    ("data/pages/bible/jeremiah/36.txt", include_str!("starter/bible/jeremiah/36.txt")),
    ("data/pages/bible/jeremiah/37.txt", include_str!("starter/bible/jeremiah/37.txt")),
    ("data/pages/bible/jeremiah/38.txt", include_str!("starter/bible/jeremiah/38.txt")),
    ("data/pages/bible/jeremiah/39.txt", include_str!("starter/bible/jeremiah/39.txt")),
    ("data/pages/bible/jeremiah/40.txt", include_str!("starter/bible/jeremiah/40.txt")),
    ("data/pages/bible/jeremiah/41.txt", include_str!("starter/bible/jeremiah/41.txt")),
    ("data/pages/bible/jeremiah/42.txt", include_str!("starter/bible/jeremiah/42.txt")),
    ("data/pages/bible/jeremiah/43.txt", include_str!("starter/bible/jeremiah/43.txt")),
    ("data/pages/bible/jeremiah/44.txt", include_str!("starter/bible/jeremiah/44.txt")),
    ("data/pages/bible/jeremiah/45.txt", include_str!("starter/bible/jeremiah/45.txt")),
    ("data/pages/bible/jeremiah/46.txt", include_str!("starter/bible/jeremiah/46.txt")),
    ("data/pages/bible/jeremiah/47.txt", include_str!("starter/bible/jeremiah/47.txt")),
    ("data/pages/bible/jeremiah/48.txt", include_str!("starter/bible/jeremiah/48.txt")),
    ("data/pages/bible/jeremiah/49.txt", include_str!("starter/bible/jeremiah/49.txt")),
    ("data/pages/bible/jeremiah/50.txt", include_str!("starter/bible/jeremiah/50.txt")),
    ("data/pages/bible/jeremiah/51.txt", include_str!("starter/bible/jeremiah/51.txt")),
    ("data/pages/bible/jeremiah/52.txt", include_str!("starter/bible/jeremiah/52.txt")),
    ("data/pages/bible/jeremiah/start.txt", include_str!("starter/bible/jeremiah/start.txt")),
    ("data/pages/bible/lamentations/1.txt", include_str!("starter/bible/lamentations/1.txt")),
    ("data/pages/bible/lamentations/2.txt", include_str!("starter/bible/lamentations/2.txt")),
    ("data/pages/bible/lamentations/3.txt", include_str!("starter/bible/lamentations/3.txt")),
    ("data/pages/bible/lamentations/4.txt", include_str!("starter/bible/lamentations/4.txt")),
    ("data/pages/bible/lamentations/5.txt", include_str!("starter/bible/lamentations/5.txt")),
    ("data/pages/bible/lamentations/start.txt", include_str!("starter/bible/lamentations/start.txt")),
    ("data/pages/bible/ezekiel/1.txt", include_str!("starter/bible/ezekiel/1.txt")),
    ("data/pages/bible/ezekiel/2.txt", include_str!("starter/bible/ezekiel/2.txt")),
    ("data/pages/bible/ezekiel/3.txt", include_str!("starter/bible/ezekiel/3.txt")),
    ("data/pages/bible/ezekiel/4.txt", include_str!("starter/bible/ezekiel/4.txt")),
    ("data/pages/bible/ezekiel/5.txt", include_str!("starter/bible/ezekiel/5.txt")),
    ("data/pages/bible/ezekiel/6.txt", include_str!("starter/bible/ezekiel/6.txt")),
    ("data/pages/bible/ezekiel/7.txt", include_str!("starter/bible/ezekiel/7.txt")),
    ("data/pages/bible/ezekiel/8.txt", include_str!("starter/bible/ezekiel/8.txt")),
    ("data/pages/bible/ezekiel/9.txt", include_str!("starter/bible/ezekiel/9.txt")),
    ("data/pages/bible/ezekiel/10.txt", include_str!("starter/bible/ezekiel/10.txt")),
    ("data/pages/bible/ezekiel/11.txt", include_str!("starter/bible/ezekiel/11.txt")),
    ("data/pages/bible/ezekiel/12.txt", include_str!("starter/bible/ezekiel/12.txt")),
    ("data/pages/bible/ezekiel/13.txt", include_str!("starter/bible/ezekiel/13.txt")),
    ("data/pages/bible/ezekiel/14.txt", include_str!("starter/bible/ezekiel/14.txt")),
    ("data/pages/bible/ezekiel/15.txt", include_str!("starter/bible/ezekiel/15.txt")),
    ("data/pages/bible/ezekiel/16.txt", include_str!("starter/bible/ezekiel/16.txt")),
    ("data/pages/bible/ezekiel/17.txt", include_str!("starter/bible/ezekiel/17.txt")),
    ("data/pages/bible/ezekiel/18.txt", include_str!("starter/bible/ezekiel/18.txt")),
    ("data/pages/bible/ezekiel/19.txt", include_str!("starter/bible/ezekiel/19.txt")),
    ("data/pages/bible/ezekiel/20.txt", include_str!("starter/bible/ezekiel/20.txt")),
    ("data/pages/bible/ezekiel/21.txt", include_str!("starter/bible/ezekiel/21.txt")),
    ("data/pages/bible/ezekiel/22.txt", include_str!("starter/bible/ezekiel/22.txt")),
    ("data/pages/bible/ezekiel/23.txt", include_str!("starter/bible/ezekiel/23.txt")),
    ("data/pages/bible/ezekiel/24.txt", include_str!("starter/bible/ezekiel/24.txt")),
    ("data/pages/bible/ezekiel/25.txt", include_str!("starter/bible/ezekiel/25.txt")),
    ("data/pages/bible/ezekiel/26.txt", include_str!("starter/bible/ezekiel/26.txt")),
    ("data/pages/bible/ezekiel/27.txt", include_str!("starter/bible/ezekiel/27.txt")),
    ("data/pages/bible/ezekiel/28.txt", include_str!("starter/bible/ezekiel/28.txt")),
    ("data/pages/bible/ezekiel/29.txt", include_str!("starter/bible/ezekiel/29.txt")),
    ("data/pages/bible/ezekiel/30.txt", include_str!("starter/bible/ezekiel/30.txt")),
    ("data/pages/bible/ezekiel/31.txt", include_str!("starter/bible/ezekiel/31.txt")),
    ("data/pages/bible/ezekiel/32.txt", include_str!("starter/bible/ezekiel/32.txt")),
    ("data/pages/bible/ezekiel/33.txt", include_str!("starter/bible/ezekiel/33.txt")),
    ("data/pages/bible/ezekiel/34.txt", include_str!("starter/bible/ezekiel/34.txt")),
    ("data/pages/bible/ezekiel/35.txt", include_str!("starter/bible/ezekiel/35.txt")),
    ("data/pages/bible/ezekiel/36.txt", include_str!("starter/bible/ezekiel/36.txt")),
    ("data/pages/bible/ezekiel/37.txt", include_str!("starter/bible/ezekiel/37.txt")),
    ("data/pages/bible/ezekiel/38.txt", include_str!("starter/bible/ezekiel/38.txt")),
    ("data/pages/bible/ezekiel/39.txt", include_str!("starter/bible/ezekiel/39.txt")),
    ("data/pages/bible/ezekiel/40.txt", include_str!("starter/bible/ezekiel/40.txt")),
    ("data/pages/bible/ezekiel/41.txt", include_str!("starter/bible/ezekiel/41.txt")),
    ("data/pages/bible/ezekiel/42.txt", include_str!("starter/bible/ezekiel/42.txt")),
    ("data/pages/bible/ezekiel/43.txt", include_str!("starter/bible/ezekiel/43.txt")),
    ("data/pages/bible/ezekiel/44.txt", include_str!("starter/bible/ezekiel/44.txt")),
    ("data/pages/bible/ezekiel/45.txt", include_str!("starter/bible/ezekiel/45.txt")),
    ("data/pages/bible/ezekiel/46.txt", include_str!("starter/bible/ezekiel/46.txt")),
    ("data/pages/bible/ezekiel/47.txt", include_str!("starter/bible/ezekiel/47.txt")),
    ("data/pages/bible/ezekiel/48.txt", include_str!("starter/bible/ezekiel/48.txt")),
    ("data/pages/bible/ezekiel/start.txt", include_str!("starter/bible/ezekiel/start.txt")),
    ("data/pages/bible/daniel/1.txt", include_str!("starter/bible/daniel/1.txt")),
    ("data/pages/bible/daniel/2.txt", include_str!("starter/bible/daniel/2.txt")),
    ("data/pages/bible/daniel/3.txt", include_str!("starter/bible/daniel/3.txt")),
    ("data/pages/bible/daniel/4.txt", include_str!("starter/bible/daniel/4.txt")),
    ("data/pages/bible/daniel/5.txt", include_str!("starter/bible/daniel/5.txt")),
    ("data/pages/bible/daniel/6.txt", include_str!("starter/bible/daniel/6.txt")),
    ("data/pages/bible/daniel/7.txt", include_str!("starter/bible/daniel/7.txt")),
    ("data/pages/bible/daniel/8.txt", include_str!("starter/bible/daniel/8.txt")),
    ("data/pages/bible/daniel/9.txt", include_str!("starter/bible/daniel/9.txt")),
    ("data/pages/bible/daniel/10.txt", include_str!("starter/bible/daniel/10.txt")),
    ("data/pages/bible/daniel/11.txt", include_str!("starter/bible/daniel/11.txt")),
    ("data/pages/bible/daniel/12.txt", include_str!("starter/bible/daniel/12.txt")),
    ("data/pages/bible/daniel/start.txt", include_str!("starter/bible/daniel/start.txt")),
    ("data/pages/bible/hosea/1.txt", include_str!("starter/bible/hosea/1.txt")),
    ("data/pages/bible/hosea/2.txt", include_str!("starter/bible/hosea/2.txt")),
    ("data/pages/bible/hosea/3.txt", include_str!("starter/bible/hosea/3.txt")),
    ("data/pages/bible/hosea/4.txt", include_str!("starter/bible/hosea/4.txt")),
    ("data/pages/bible/hosea/5.txt", include_str!("starter/bible/hosea/5.txt")),
    ("data/pages/bible/hosea/6.txt", include_str!("starter/bible/hosea/6.txt")),
    ("data/pages/bible/hosea/7.txt", include_str!("starter/bible/hosea/7.txt")),
    ("data/pages/bible/hosea/8.txt", include_str!("starter/bible/hosea/8.txt")),
    ("data/pages/bible/hosea/9.txt", include_str!("starter/bible/hosea/9.txt")),
    ("data/pages/bible/hosea/10.txt", include_str!("starter/bible/hosea/10.txt")),
    ("data/pages/bible/hosea/11.txt", include_str!("starter/bible/hosea/11.txt")),
    ("data/pages/bible/hosea/12.txt", include_str!("starter/bible/hosea/12.txt")),
    ("data/pages/bible/hosea/13.txt", include_str!("starter/bible/hosea/13.txt")),
    ("data/pages/bible/hosea/14.txt", include_str!("starter/bible/hosea/14.txt")),
    ("data/pages/bible/hosea/start.txt", include_str!("starter/bible/hosea/start.txt")),
    ("data/pages/bible/joel/1.txt", include_str!("starter/bible/joel/1.txt")),
    ("data/pages/bible/joel/2.txt", include_str!("starter/bible/joel/2.txt")),
    ("data/pages/bible/joel/3.txt", include_str!("starter/bible/joel/3.txt")),
    ("data/pages/bible/joel/start.txt", include_str!("starter/bible/joel/start.txt")),
    ("data/pages/bible/amos/1.txt", include_str!("starter/bible/amos/1.txt")),
    ("data/pages/bible/amos/2.txt", include_str!("starter/bible/amos/2.txt")),
    ("data/pages/bible/amos/3.txt", include_str!("starter/bible/amos/3.txt")),
    ("data/pages/bible/amos/4.txt", include_str!("starter/bible/amos/4.txt")),
    ("data/pages/bible/amos/5.txt", include_str!("starter/bible/amos/5.txt")),
    ("data/pages/bible/amos/6.txt", include_str!("starter/bible/amos/6.txt")),
    ("data/pages/bible/amos/7.txt", include_str!("starter/bible/amos/7.txt")),
    ("data/pages/bible/amos/8.txt", include_str!("starter/bible/amos/8.txt")),
    ("data/pages/bible/amos/9.txt", include_str!("starter/bible/amos/9.txt")),
    ("data/pages/bible/amos/start.txt", include_str!("starter/bible/amos/start.txt")),
    ("data/pages/bible/obadiah/1.txt", include_str!("starter/bible/obadiah/1.txt")),
    ("data/pages/bible/obadiah/start.txt", include_str!("starter/bible/obadiah/start.txt")),
    ("data/pages/bible/jonah/1.txt", include_str!("starter/bible/jonah/1.txt")),
    ("data/pages/bible/jonah/2.txt", include_str!("starter/bible/jonah/2.txt")),
    ("data/pages/bible/jonah/3.txt", include_str!("starter/bible/jonah/3.txt")),
    ("data/pages/bible/jonah/4.txt", include_str!("starter/bible/jonah/4.txt")),
    ("data/pages/bible/jonah/start.txt", include_str!("starter/bible/jonah/start.txt")),
    ("data/pages/bible/micah/1.txt", include_str!("starter/bible/micah/1.txt")),
    ("data/pages/bible/micah/2.txt", include_str!("starter/bible/micah/2.txt")),
    ("data/pages/bible/micah/3.txt", include_str!("starter/bible/micah/3.txt")),
    ("data/pages/bible/micah/4.txt", include_str!("starter/bible/micah/4.txt")),
    ("data/pages/bible/micah/5.txt", include_str!("starter/bible/micah/5.txt")),
    ("data/pages/bible/micah/6.txt", include_str!("starter/bible/micah/6.txt")),
    ("data/pages/bible/micah/7.txt", include_str!("starter/bible/micah/7.txt")),
    ("data/pages/bible/micah/start.txt", include_str!("starter/bible/micah/start.txt")),
    ("data/pages/bible/nahum/1.txt", include_str!("starter/bible/nahum/1.txt")),
    ("data/pages/bible/nahum/2.txt", include_str!("starter/bible/nahum/2.txt")),
    ("data/pages/bible/nahum/3.txt", include_str!("starter/bible/nahum/3.txt")),
    ("data/pages/bible/nahum/start.txt", include_str!("starter/bible/nahum/start.txt")),
    ("data/pages/bible/habakkuk/1.txt", include_str!("starter/bible/habakkuk/1.txt")),
    ("data/pages/bible/habakkuk/2.txt", include_str!("starter/bible/habakkuk/2.txt")),
    ("data/pages/bible/habakkuk/3.txt", include_str!("starter/bible/habakkuk/3.txt")),
    ("data/pages/bible/habakkuk/start.txt", include_str!("starter/bible/habakkuk/start.txt")),
    ("data/pages/bible/zephaniah/1.txt", include_str!("starter/bible/zephaniah/1.txt")),
    ("data/pages/bible/zephaniah/2.txt", include_str!("starter/bible/zephaniah/2.txt")),
    ("data/pages/bible/zephaniah/3.txt", include_str!("starter/bible/zephaniah/3.txt")),
    ("data/pages/bible/zephaniah/start.txt", include_str!("starter/bible/zephaniah/start.txt")),
    ("data/pages/bible/haggai/1.txt", include_str!("starter/bible/haggai/1.txt")),
    ("data/pages/bible/haggai/2.txt", include_str!("starter/bible/haggai/2.txt")),
    ("data/pages/bible/haggai/start.txt", include_str!("starter/bible/haggai/start.txt")),
    ("data/pages/bible/zechariah/1.txt", include_str!("starter/bible/zechariah/1.txt")),
    ("data/pages/bible/zechariah/2.txt", include_str!("starter/bible/zechariah/2.txt")),
    ("data/pages/bible/zechariah/3.txt", include_str!("starter/bible/zechariah/3.txt")),
    ("data/pages/bible/zechariah/4.txt", include_str!("starter/bible/zechariah/4.txt")),
    ("data/pages/bible/zechariah/5.txt", include_str!("starter/bible/zechariah/5.txt")),
    ("data/pages/bible/zechariah/6.txt", include_str!("starter/bible/zechariah/6.txt")),
    ("data/pages/bible/zechariah/7.txt", include_str!("starter/bible/zechariah/7.txt")),
    ("data/pages/bible/zechariah/8.txt", include_str!("starter/bible/zechariah/8.txt")),
    ("data/pages/bible/zechariah/9.txt", include_str!("starter/bible/zechariah/9.txt")),
    ("data/pages/bible/zechariah/10.txt", include_str!("starter/bible/zechariah/10.txt")),
    ("data/pages/bible/zechariah/11.txt", include_str!("starter/bible/zechariah/11.txt")),
    ("data/pages/bible/zechariah/12.txt", include_str!("starter/bible/zechariah/12.txt")),
    ("data/pages/bible/zechariah/13.txt", include_str!("starter/bible/zechariah/13.txt")),
    ("data/pages/bible/zechariah/14.txt", include_str!("starter/bible/zechariah/14.txt")),
    ("data/pages/bible/zechariah/start.txt", include_str!("starter/bible/zechariah/start.txt")),
    ("data/pages/bible/malachi/1.txt", include_str!("starter/bible/malachi/1.txt")),
    ("data/pages/bible/malachi/2.txt", include_str!("starter/bible/malachi/2.txt")),
    ("data/pages/bible/malachi/3.txt", include_str!("starter/bible/malachi/3.txt")),
    ("data/pages/bible/malachi/4.txt", include_str!("starter/bible/malachi/4.txt")),
    ("data/pages/bible/malachi/start.txt", include_str!("starter/bible/malachi/start.txt")),
    ("data/pages/bible/matthew/1.txt", include_str!("starter/bible/matthew/1.txt")),
    ("data/pages/bible/matthew/2.txt", include_str!("starter/bible/matthew/2.txt")),
    ("data/pages/bible/matthew/3.txt", include_str!("starter/bible/matthew/3.txt")),
    ("data/pages/bible/matthew/4.txt", include_str!("starter/bible/matthew/4.txt")),
    ("data/pages/bible/matthew/5.txt", include_str!("starter/bible/matthew/5.txt")),
    ("data/pages/bible/matthew/6.txt", include_str!("starter/bible/matthew/6.txt")),
    ("data/pages/bible/matthew/7.txt", include_str!("starter/bible/matthew/7.txt")),
    ("data/pages/bible/matthew/8.txt", include_str!("starter/bible/matthew/8.txt")),
    ("data/pages/bible/matthew/9.txt", include_str!("starter/bible/matthew/9.txt")),
    ("data/pages/bible/matthew/10.txt", include_str!("starter/bible/matthew/10.txt")),
    ("data/pages/bible/matthew/11.txt", include_str!("starter/bible/matthew/11.txt")),
    ("data/pages/bible/matthew/12.txt", include_str!("starter/bible/matthew/12.txt")),
    ("data/pages/bible/matthew/13.txt", include_str!("starter/bible/matthew/13.txt")),
    ("data/pages/bible/matthew/14.txt", include_str!("starter/bible/matthew/14.txt")),
    ("data/pages/bible/matthew/15.txt", include_str!("starter/bible/matthew/15.txt")),
    ("data/pages/bible/matthew/16.txt", include_str!("starter/bible/matthew/16.txt")),
    ("data/pages/bible/matthew/17.txt", include_str!("starter/bible/matthew/17.txt")),
    ("data/pages/bible/matthew/18.txt", include_str!("starter/bible/matthew/18.txt")),
    ("data/pages/bible/matthew/19.txt", include_str!("starter/bible/matthew/19.txt")),
    ("data/pages/bible/matthew/20.txt", include_str!("starter/bible/matthew/20.txt")),
    ("data/pages/bible/matthew/21.txt", include_str!("starter/bible/matthew/21.txt")),
    ("data/pages/bible/matthew/22.txt", include_str!("starter/bible/matthew/22.txt")),
    ("data/pages/bible/matthew/23.txt", include_str!("starter/bible/matthew/23.txt")),
    ("data/pages/bible/matthew/24.txt", include_str!("starter/bible/matthew/24.txt")),
    ("data/pages/bible/matthew/25.txt", include_str!("starter/bible/matthew/25.txt")),
    ("data/pages/bible/matthew/26.txt", include_str!("starter/bible/matthew/26.txt")),
    ("data/pages/bible/matthew/27.txt", include_str!("starter/bible/matthew/27.txt")),
    ("data/pages/bible/matthew/28.txt", include_str!("starter/bible/matthew/28.txt")),
    ("data/pages/bible/matthew/start.txt", include_str!("starter/bible/matthew/start.txt")),
    ("data/pages/bible/mark/1.txt", include_str!("starter/bible/mark/1.txt")),
    ("data/pages/bible/mark/2.txt", include_str!("starter/bible/mark/2.txt")),
    ("data/pages/bible/mark/3.txt", include_str!("starter/bible/mark/3.txt")),
    ("data/pages/bible/mark/4.txt", include_str!("starter/bible/mark/4.txt")),
    ("data/pages/bible/mark/5.txt", include_str!("starter/bible/mark/5.txt")),
    ("data/pages/bible/mark/6.txt", include_str!("starter/bible/mark/6.txt")),
    ("data/pages/bible/mark/7.txt", include_str!("starter/bible/mark/7.txt")),
    ("data/pages/bible/mark/8.txt", include_str!("starter/bible/mark/8.txt")),
    ("data/pages/bible/mark/9.txt", include_str!("starter/bible/mark/9.txt")),
    ("data/pages/bible/mark/10.txt", include_str!("starter/bible/mark/10.txt")),
    ("data/pages/bible/mark/11.txt", include_str!("starter/bible/mark/11.txt")),
    ("data/pages/bible/mark/12.txt", include_str!("starter/bible/mark/12.txt")),
    ("data/pages/bible/mark/13.txt", include_str!("starter/bible/mark/13.txt")),
    ("data/pages/bible/mark/14.txt", include_str!("starter/bible/mark/14.txt")),
    ("data/pages/bible/mark/15.txt", include_str!("starter/bible/mark/15.txt")),
    ("data/pages/bible/mark/16.txt", include_str!("starter/bible/mark/16.txt")),
    ("data/pages/bible/mark/start.txt", include_str!("starter/bible/mark/start.txt")),
    ("data/pages/bible/luke/1.txt", include_str!("starter/bible/luke/1.txt")),
    ("data/pages/bible/luke/2.txt", include_str!("starter/bible/luke/2.txt")),
    ("data/pages/bible/luke/3.txt", include_str!("starter/bible/luke/3.txt")),
    ("data/pages/bible/luke/4.txt", include_str!("starter/bible/luke/4.txt")),
    ("data/pages/bible/luke/5.txt", include_str!("starter/bible/luke/5.txt")),
    ("data/pages/bible/luke/6.txt", include_str!("starter/bible/luke/6.txt")),
    ("data/pages/bible/luke/7.txt", include_str!("starter/bible/luke/7.txt")),
    ("data/pages/bible/luke/8.txt", include_str!("starter/bible/luke/8.txt")),
    ("data/pages/bible/luke/9.txt", include_str!("starter/bible/luke/9.txt")),
    ("data/pages/bible/luke/10.txt", include_str!("starter/bible/luke/10.txt")),
    ("data/pages/bible/luke/11.txt", include_str!("starter/bible/luke/11.txt")),
    ("data/pages/bible/luke/12.txt", include_str!("starter/bible/luke/12.txt")),
    ("data/pages/bible/luke/13.txt", include_str!("starter/bible/luke/13.txt")),
    ("data/pages/bible/luke/14.txt", include_str!("starter/bible/luke/14.txt")),
    ("data/pages/bible/luke/15.txt", include_str!("starter/bible/luke/15.txt")),
    ("data/pages/bible/luke/16.txt", include_str!("starter/bible/luke/16.txt")),
    ("data/pages/bible/luke/17.txt", include_str!("starter/bible/luke/17.txt")),
    ("data/pages/bible/luke/18.txt", include_str!("starter/bible/luke/18.txt")),
    ("data/pages/bible/luke/19.txt", include_str!("starter/bible/luke/19.txt")),
    ("data/pages/bible/luke/20.txt", include_str!("starter/bible/luke/20.txt")),
    ("data/pages/bible/luke/21.txt", include_str!("starter/bible/luke/21.txt")),
    ("data/pages/bible/luke/22.txt", include_str!("starter/bible/luke/22.txt")),
    ("data/pages/bible/luke/23.txt", include_str!("starter/bible/luke/23.txt")),
    ("data/pages/bible/luke/24.txt", include_str!("starter/bible/luke/24.txt")),
    ("data/pages/bible/luke/start.txt", include_str!("starter/bible/luke/start.txt")),
    ("data/pages/bible/john/1.txt", include_str!("starter/bible/john/1.txt")),
    ("data/pages/bible/john/2.txt", include_str!("starter/bible/john/2.txt")),
    ("data/pages/bible/john/3.txt", include_str!("starter/bible/john/3.txt")),
    ("data/pages/bible/john/4.txt", include_str!("starter/bible/john/4.txt")),
    ("data/pages/bible/john/5.txt", include_str!("starter/bible/john/5.txt")),
    ("data/pages/bible/john/6.txt", include_str!("starter/bible/john/6.txt")),
    ("data/pages/bible/john/7.txt", include_str!("starter/bible/john/7.txt")),
    ("data/pages/bible/john/8.txt", include_str!("starter/bible/john/8.txt")),
    ("data/pages/bible/john/9.txt", include_str!("starter/bible/john/9.txt")),
    ("data/pages/bible/john/10.txt", include_str!("starter/bible/john/10.txt")),
    ("data/pages/bible/john/11.txt", include_str!("starter/bible/john/11.txt")),
    ("data/pages/bible/john/12.txt", include_str!("starter/bible/john/12.txt")),
    ("data/pages/bible/john/13.txt", include_str!("starter/bible/john/13.txt")),
    ("data/pages/bible/john/14.txt", include_str!("starter/bible/john/14.txt")),
    ("data/pages/bible/john/15.txt", include_str!("starter/bible/john/15.txt")),
    ("data/pages/bible/john/16.txt", include_str!("starter/bible/john/16.txt")),
    ("data/pages/bible/john/17.txt", include_str!("starter/bible/john/17.txt")),
    ("data/pages/bible/john/18.txt", include_str!("starter/bible/john/18.txt")),
    ("data/pages/bible/john/19.txt", include_str!("starter/bible/john/19.txt")),
    ("data/pages/bible/john/20.txt", include_str!("starter/bible/john/20.txt")),
    ("data/pages/bible/john/21.txt", include_str!("starter/bible/john/21.txt")),
    ("data/pages/bible/john/start.txt", include_str!("starter/bible/john/start.txt")),
    ("data/pages/bible/acts/1.txt", include_str!("starter/bible/acts/1.txt")),
    ("data/pages/bible/acts/2.txt", include_str!("starter/bible/acts/2.txt")),
    ("data/pages/bible/acts/3.txt", include_str!("starter/bible/acts/3.txt")),
    ("data/pages/bible/acts/4.txt", include_str!("starter/bible/acts/4.txt")),
    ("data/pages/bible/acts/5.txt", include_str!("starter/bible/acts/5.txt")),
    ("data/pages/bible/acts/6.txt", include_str!("starter/bible/acts/6.txt")),
    ("data/pages/bible/acts/7.txt", include_str!("starter/bible/acts/7.txt")),
    ("data/pages/bible/acts/8.txt", include_str!("starter/bible/acts/8.txt")),
    ("data/pages/bible/acts/9.txt", include_str!("starter/bible/acts/9.txt")),
    ("data/pages/bible/acts/10.txt", include_str!("starter/bible/acts/10.txt")),
    ("data/pages/bible/acts/11.txt", include_str!("starter/bible/acts/11.txt")),
    ("data/pages/bible/acts/12.txt", include_str!("starter/bible/acts/12.txt")),
    ("data/pages/bible/acts/13.txt", include_str!("starter/bible/acts/13.txt")),
    ("data/pages/bible/acts/14.txt", include_str!("starter/bible/acts/14.txt")),
    ("data/pages/bible/acts/15.txt", include_str!("starter/bible/acts/15.txt")),
    ("data/pages/bible/acts/16.txt", include_str!("starter/bible/acts/16.txt")),
    ("data/pages/bible/acts/17.txt", include_str!("starter/bible/acts/17.txt")),
    ("data/pages/bible/acts/18.txt", include_str!("starter/bible/acts/18.txt")),
    ("data/pages/bible/acts/19.txt", include_str!("starter/bible/acts/19.txt")),
    ("data/pages/bible/acts/20.txt", include_str!("starter/bible/acts/20.txt")),
    ("data/pages/bible/acts/21.txt", include_str!("starter/bible/acts/21.txt")),
    ("data/pages/bible/acts/22.txt", include_str!("starter/bible/acts/22.txt")),
    ("data/pages/bible/acts/23.txt", include_str!("starter/bible/acts/23.txt")),
    ("data/pages/bible/acts/24.txt", include_str!("starter/bible/acts/24.txt")),
    ("data/pages/bible/acts/25.txt", include_str!("starter/bible/acts/25.txt")),
    ("data/pages/bible/acts/26.txt", include_str!("starter/bible/acts/26.txt")),
    ("data/pages/bible/acts/27.txt", include_str!("starter/bible/acts/27.txt")),
    ("data/pages/bible/acts/28.txt", include_str!("starter/bible/acts/28.txt")),
    ("data/pages/bible/acts/start.txt", include_str!("starter/bible/acts/start.txt")),
    ("data/pages/bible/romans/1.txt", include_str!("starter/bible/romans/1.txt")),
    ("data/pages/bible/romans/2.txt", include_str!("starter/bible/romans/2.txt")),
    ("data/pages/bible/romans/3.txt", include_str!("starter/bible/romans/3.txt")),
    ("data/pages/bible/romans/4.txt", include_str!("starter/bible/romans/4.txt")),
    ("data/pages/bible/romans/5.txt", include_str!("starter/bible/romans/5.txt")),
    ("data/pages/bible/romans/6.txt", include_str!("starter/bible/romans/6.txt")),
    ("data/pages/bible/romans/7.txt", include_str!("starter/bible/romans/7.txt")),
    ("data/pages/bible/romans/8.txt", include_str!("starter/bible/romans/8.txt")),
    ("data/pages/bible/romans/9.txt", include_str!("starter/bible/romans/9.txt")),
    ("data/pages/bible/romans/10.txt", include_str!("starter/bible/romans/10.txt")),
    ("data/pages/bible/romans/11.txt", include_str!("starter/bible/romans/11.txt")),
    ("data/pages/bible/romans/12.txt", include_str!("starter/bible/romans/12.txt")),
    ("data/pages/bible/romans/13.txt", include_str!("starter/bible/romans/13.txt")),
    ("data/pages/bible/romans/14.txt", include_str!("starter/bible/romans/14.txt")),
    ("data/pages/bible/romans/15.txt", include_str!("starter/bible/romans/15.txt")),
    ("data/pages/bible/romans/16.txt", include_str!("starter/bible/romans/16.txt")),
    ("data/pages/bible/romans/start.txt", include_str!("starter/bible/romans/start.txt")),
    ("data/pages/bible/1_corinthians/1.txt", include_str!("starter/bible/1_corinthians/1.txt")),
    ("data/pages/bible/1_corinthians/2.txt", include_str!("starter/bible/1_corinthians/2.txt")),
    ("data/pages/bible/1_corinthians/3.txt", include_str!("starter/bible/1_corinthians/3.txt")),
    ("data/pages/bible/1_corinthians/4.txt", include_str!("starter/bible/1_corinthians/4.txt")),
    ("data/pages/bible/1_corinthians/5.txt", include_str!("starter/bible/1_corinthians/5.txt")),
    ("data/pages/bible/1_corinthians/6.txt", include_str!("starter/bible/1_corinthians/6.txt")),
    ("data/pages/bible/1_corinthians/7.txt", include_str!("starter/bible/1_corinthians/7.txt")),
    ("data/pages/bible/1_corinthians/8.txt", include_str!("starter/bible/1_corinthians/8.txt")),
    ("data/pages/bible/1_corinthians/9.txt", include_str!("starter/bible/1_corinthians/9.txt")),
    ("data/pages/bible/1_corinthians/10.txt", include_str!("starter/bible/1_corinthians/10.txt")),
    ("data/pages/bible/1_corinthians/11.txt", include_str!("starter/bible/1_corinthians/11.txt")),
    ("data/pages/bible/1_corinthians/12.txt", include_str!("starter/bible/1_corinthians/12.txt")),
    ("data/pages/bible/1_corinthians/13.txt", include_str!("starter/bible/1_corinthians/13.txt")),
    ("data/pages/bible/1_corinthians/14.txt", include_str!("starter/bible/1_corinthians/14.txt")),
    ("data/pages/bible/1_corinthians/15.txt", include_str!("starter/bible/1_corinthians/15.txt")),
    ("data/pages/bible/1_corinthians/16.txt", include_str!("starter/bible/1_corinthians/16.txt")),
    ("data/pages/bible/1_corinthians/start.txt", include_str!("starter/bible/1_corinthians/start.txt")),
    ("data/pages/bible/2_corinthians/1.txt", include_str!("starter/bible/2_corinthians/1.txt")),
    ("data/pages/bible/2_corinthians/2.txt", include_str!("starter/bible/2_corinthians/2.txt")),
    ("data/pages/bible/2_corinthians/3.txt", include_str!("starter/bible/2_corinthians/3.txt")),
    ("data/pages/bible/2_corinthians/4.txt", include_str!("starter/bible/2_corinthians/4.txt")),
    ("data/pages/bible/2_corinthians/5.txt", include_str!("starter/bible/2_corinthians/5.txt")),
    ("data/pages/bible/2_corinthians/6.txt", include_str!("starter/bible/2_corinthians/6.txt")),
    ("data/pages/bible/2_corinthians/7.txt", include_str!("starter/bible/2_corinthians/7.txt")),
    ("data/pages/bible/2_corinthians/8.txt", include_str!("starter/bible/2_corinthians/8.txt")),
    ("data/pages/bible/2_corinthians/9.txt", include_str!("starter/bible/2_corinthians/9.txt")),
    ("data/pages/bible/2_corinthians/10.txt", include_str!("starter/bible/2_corinthians/10.txt")),
    ("data/pages/bible/2_corinthians/11.txt", include_str!("starter/bible/2_corinthians/11.txt")),
    ("data/pages/bible/2_corinthians/12.txt", include_str!("starter/bible/2_corinthians/12.txt")),
    ("data/pages/bible/2_corinthians/13.txt", include_str!("starter/bible/2_corinthians/13.txt")),
    ("data/pages/bible/2_corinthians/start.txt", include_str!("starter/bible/2_corinthians/start.txt")),
    ("data/pages/bible/galatians/1.txt", include_str!("starter/bible/galatians/1.txt")),
    ("data/pages/bible/galatians/2.txt", include_str!("starter/bible/galatians/2.txt")),
    ("data/pages/bible/galatians/3.txt", include_str!("starter/bible/galatians/3.txt")),
    ("data/pages/bible/galatians/4.txt", include_str!("starter/bible/galatians/4.txt")),
    ("data/pages/bible/galatians/5.txt", include_str!("starter/bible/galatians/5.txt")),
    ("data/pages/bible/galatians/6.txt", include_str!("starter/bible/galatians/6.txt")),
    ("data/pages/bible/galatians/start.txt", include_str!("starter/bible/galatians/start.txt")),
    ("data/pages/bible/ephesians/1.txt", include_str!("starter/bible/ephesians/1.txt")),
    ("data/pages/bible/ephesians/2.txt", include_str!("starter/bible/ephesians/2.txt")),
    ("data/pages/bible/ephesians/3.txt", include_str!("starter/bible/ephesians/3.txt")),
    ("data/pages/bible/ephesians/4.txt", include_str!("starter/bible/ephesians/4.txt")),
    ("data/pages/bible/ephesians/5.txt", include_str!("starter/bible/ephesians/5.txt")),
    ("data/pages/bible/ephesians/6.txt", include_str!("starter/bible/ephesians/6.txt")),
    ("data/pages/bible/ephesians/start.txt", include_str!("starter/bible/ephesians/start.txt")),
    ("data/pages/bible/philippians/1.txt", include_str!("starter/bible/philippians/1.txt")),
    ("data/pages/bible/philippians/2.txt", include_str!("starter/bible/philippians/2.txt")),
    ("data/pages/bible/philippians/3.txt", include_str!("starter/bible/philippians/3.txt")),
    ("data/pages/bible/philippians/4.txt", include_str!("starter/bible/philippians/4.txt")),
    ("data/pages/bible/philippians/start.txt", include_str!("starter/bible/philippians/start.txt")),
    ("data/pages/bible/colossians/1.txt", include_str!("starter/bible/colossians/1.txt")),
    ("data/pages/bible/colossians/2.txt", include_str!("starter/bible/colossians/2.txt")),
    ("data/pages/bible/colossians/3.txt", include_str!("starter/bible/colossians/3.txt")),
    ("data/pages/bible/colossians/4.txt", include_str!("starter/bible/colossians/4.txt")),
    ("data/pages/bible/colossians/start.txt", include_str!("starter/bible/colossians/start.txt")),
    ("data/pages/bible/1_thessalonians/1.txt", include_str!("starter/bible/1_thessalonians/1.txt")),
    ("data/pages/bible/1_thessalonians/2.txt", include_str!("starter/bible/1_thessalonians/2.txt")),
    ("data/pages/bible/1_thessalonians/3.txt", include_str!("starter/bible/1_thessalonians/3.txt")),
    ("data/pages/bible/1_thessalonians/4.txt", include_str!("starter/bible/1_thessalonians/4.txt")),
    ("data/pages/bible/1_thessalonians/5.txt", include_str!("starter/bible/1_thessalonians/5.txt")),
    ("data/pages/bible/1_thessalonians/start.txt", include_str!("starter/bible/1_thessalonians/start.txt")),
    ("data/pages/bible/2_thessalonians/1.txt", include_str!("starter/bible/2_thessalonians/1.txt")),
    ("data/pages/bible/2_thessalonians/2.txt", include_str!("starter/bible/2_thessalonians/2.txt")),
    ("data/pages/bible/2_thessalonians/3.txt", include_str!("starter/bible/2_thessalonians/3.txt")),
    ("data/pages/bible/2_thessalonians/start.txt", include_str!("starter/bible/2_thessalonians/start.txt")),
    ("data/pages/bible/1_timothy/1.txt", include_str!("starter/bible/1_timothy/1.txt")),
    ("data/pages/bible/1_timothy/2.txt", include_str!("starter/bible/1_timothy/2.txt")),
    ("data/pages/bible/1_timothy/3.txt", include_str!("starter/bible/1_timothy/3.txt")),
    ("data/pages/bible/1_timothy/4.txt", include_str!("starter/bible/1_timothy/4.txt")),
    ("data/pages/bible/1_timothy/5.txt", include_str!("starter/bible/1_timothy/5.txt")),
    ("data/pages/bible/1_timothy/6.txt", include_str!("starter/bible/1_timothy/6.txt")),
    ("data/pages/bible/1_timothy/start.txt", include_str!("starter/bible/1_timothy/start.txt")),
    ("data/pages/bible/2_timothy/1.txt", include_str!("starter/bible/2_timothy/1.txt")),
    ("data/pages/bible/2_timothy/2.txt", include_str!("starter/bible/2_timothy/2.txt")),
    ("data/pages/bible/2_timothy/3.txt", include_str!("starter/bible/2_timothy/3.txt")),
    ("data/pages/bible/2_timothy/4.txt", include_str!("starter/bible/2_timothy/4.txt")),
    ("data/pages/bible/2_timothy/start.txt", include_str!("starter/bible/2_timothy/start.txt")),
    ("data/pages/bible/titus/1.txt", include_str!("starter/bible/titus/1.txt")),
    ("data/pages/bible/titus/2.txt", include_str!("starter/bible/titus/2.txt")),
    ("data/pages/bible/titus/3.txt", include_str!("starter/bible/titus/3.txt")),
    ("data/pages/bible/titus/start.txt", include_str!("starter/bible/titus/start.txt")),
    ("data/pages/bible/philemon/1.txt", include_str!("starter/bible/philemon/1.txt")),
    ("data/pages/bible/philemon/start.txt", include_str!("starter/bible/philemon/start.txt")),
    ("data/pages/bible/hebrews/1.txt", include_str!("starter/bible/hebrews/1.txt")),
    ("data/pages/bible/hebrews/2.txt", include_str!("starter/bible/hebrews/2.txt")),
    ("data/pages/bible/hebrews/3.txt", include_str!("starter/bible/hebrews/3.txt")),
    ("data/pages/bible/hebrews/4.txt", include_str!("starter/bible/hebrews/4.txt")),
    ("data/pages/bible/hebrews/5.txt", include_str!("starter/bible/hebrews/5.txt")),
    ("data/pages/bible/hebrews/6.txt", include_str!("starter/bible/hebrews/6.txt")),
    ("data/pages/bible/hebrews/7.txt", include_str!("starter/bible/hebrews/7.txt")),
    ("data/pages/bible/hebrews/8.txt", include_str!("starter/bible/hebrews/8.txt")),
    ("data/pages/bible/hebrews/9.txt", include_str!("starter/bible/hebrews/9.txt")),
    ("data/pages/bible/hebrews/10.txt", include_str!("starter/bible/hebrews/10.txt")),
    ("data/pages/bible/hebrews/11.txt", include_str!("starter/bible/hebrews/11.txt")),
    ("data/pages/bible/hebrews/12.txt", include_str!("starter/bible/hebrews/12.txt")),
    ("data/pages/bible/hebrews/13.txt", include_str!("starter/bible/hebrews/13.txt")),
    ("data/pages/bible/hebrews/start.txt", include_str!("starter/bible/hebrews/start.txt")),
    ("data/pages/bible/james/1.txt", include_str!("starter/bible/james/1.txt")),
    ("data/pages/bible/james/2.txt", include_str!("starter/bible/james/2.txt")),
    ("data/pages/bible/james/3.txt", include_str!("starter/bible/james/3.txt")),
    ("data/pages/bible/james/4.txt", include_str!("starter/bible/james/4.txt")),
    ("data/pages/bible/james/5.txt", include_str!("starter/bible/james/5.txt")),
    ("data/pages/bible/james/start.txt", include_str!("starter/bible/james/start.txt")),
    ("data/pages/bible/1_peter/1.txt", include_str!("starter/bible/1_peter/1.txt")),
    ("data/pages/bible/1_peter/2.txt", include_str!("starter/bible/1_peter/2.txt")),
    ("data/pages/bible/1_peter/3.txt", include_str!("starter/bible/1_peter/3.txt")),
    ("data/pages/bible/1_peter/4.txt", include_str!("starter/bible/1_peter/4.txt")),
    ("data/pages/bible/1_peter/5.txt", include_str!("starter/bible/1_peter/5.txt")),
    ("data/pages/bible/1_peter/start.txt", include_str!("starter/bible/1_peter/start.txt")),
    ("data/pages/bible/2_peter/1.txt", include_str!("starter/bible/2_peter/1.txt")),
    ("data/pages/bible/2_peter/2.txt", include_str!("starter/bible/2_peter/2.txt")),
    ("data/pages/bible/2_peter/3.txt", include_str!("starter/bible/2_peter/3.txt")),
    ("data/pages/bible/2_peter/start.txt", include_str!("starter/bible/2_peter/start.txt")),
    ("data/pages/bible/1_john/1.txt", include_str!("starter/bible/1_john/1.txt")),
    ("data/pages/bible/1_john/2.txt", include_str!("starter/bible/1_john/2.txt")),
    ("data/pages/bible/1_john/3.txt", include_str!("starter/bible/1_john/3.txt")),
    ("data/pages/bible/1_john/4.txt", include_str!("starter/bible/1_john/4.txt")),
    ("data/pages/bible/1_john/5.txt", include_str!("starter/bible/1_john/5.txt")),
    ("data/pages/bible/1_john/start.txt", include_str!("starter/bible/1_john/start.txt")),
    ("data/pages/bible/2_john/1.txt", include_str!("starter/bible/2_john/1.txt")),
    ("data/pages/bible/2_john/start.txt", include_str!("starter/bible/2_john/start.txt")),
    ("data/pages/bible/3_john/1.txt", include_str!("starter/bible/3_john/1.txt")),
    ("data/pages/bible/3_john/start.txt", include_str!("starter/bible/3_john/start.txt")),
    ("data/pages/bible/jude/1.txt", include_str!("starter/bible/jude/1.txt")),
    ("data/pages/bible/jude/start.txt", include_str!("starter/bible/jude/start.txt")),
    ("data/pages/bible/revelation/1.txt", include_str!("starter/bible/revelation/1.txt")),
    ("data/pages/bible/revelation/2.txt", include_str!("starter/bible/revelation/2.txt")),
    ("data/pages/bible/revelation/3.txt", include_str!("starter/bible/revelation/3.txt")),
    ("data/pages/bible/revelation/4.txt", include_str!("starter/bible/revelation/4.txt")),
    ("data/pages/bible/revelation/5.txt", include_str!("starter/bible/revelation/5.txt")),
    ("data/pages/bible/revelation/6.txt", include_str!("starter/bible/revelation/6.txt")),
    ("data/pages/bible/revelation/7.txt", include_str!("starter/bible/revelation/7.txt")),
    ("data/pages/bible/revelation/8.txt", include_str!("starter/bible/revelation/8.txt")),
    ("data/pages/bible/revelation/9.txt", include_str!("starter/bible/revelation/9.txt")),
    ("data/pages/bible/revelation/10.txt", include_str!("starter/bible/revelation/10.txt")),
    ("data/pages/bible/revelation/11.txt", include_str!("starter/bible/revelation/11.txt")),
    ("data/pages/bible/revelation/12.txt", include_str!("starter/bible/revelation/12.txt")),
    ("data/pages/bible/revelation/13.txt", include_str!("starter/bible/revelation/13.txt")),
    ("data/pages/bible/revelation/14.txt", include_str!("starter/bible/revelation/14.txt")),
    ("data/pages/bible/revelation/15.txt", include_str!("starter/bible/revelation/15.txt")),
    ("data/pages/bible/revelation/16.txt", include_str!("starter/bible/revelation/16.txt")),
    ("data/pages/bible/revelation/17.txt", include_str!("starter/bible/revelation/17.txt")),
    ("data/pages/bible/revelation/18.txt", include_str!("starter/bible/revelation/18.txt")),
    ("data/pages/bible/revelation/19.txt", include_str!("starter/bible/revelation/19.txt")),
    ("data/pages/bible/revelation/20.txt", include_str!("starter/bible/revelation/20.txt")),
    ("data/pages/bible/revelation/21.txt", include_str!("starter/bible/revelation/21.txt")),
    ("data/pages/bible/revelation/22.txt", include_str!("starter/bible/revelation/22.txt")),
    ("data/pages/bible/revelation/start.txt", include_str!("starter/bible/revelation/start.txt")),
    ("data/pages/bible/start.txt", include_str!("starter/bible/start.txt")),
    ("data/pages/bible/notes/start.txt", include_str!("starter/bible/notes/start.txt")),
    ("data/pages/bible/notes/_template.txt", include_str!("starter/bible/notes/_template.txt")),
    ("data/pages/bible/notes/topics.txt", include_str!("starter/bible/notes/topics.txt")),
    ("data/pages/bible/notes/structured.txt", include_str!("starter/bible/notes/structured.txt")),
    ("data/pages/bible/notes/setup.txt", include_str!("starter/bible/notes/setup.txt")),
    ("data/pages/bible/notes/entry/_template.txt", include_str!("starter/bible/notes/entry/_template.txt")),
    ("data/media/wiki/bible_study_schema.json", include_str!("starter/bible/study_schema.json")),
    ("data/pages/bible/genesis/1.txt", include_str!("starter/bible/genesis/1.txt")),
    ("data/pages/bible/genesis/2.txt", include_str!("starter/bible/genesis/2.txt")),
    ("data/pages/bible/genesis/3.txt", include_str!("starter/bible/genesis/3.txt")),
    ("data/pages/bible/genesis/4.txt", include_str!("starter/bible/genesis/4.txt")),
    ("data/pages/bible/genesis/5.txt", include_str!("starter/bible/genesis/5.txt")),
    ("data/pages/bible/genesis/6.txt", include_str!("starter/bible/genesis/6.txt")),
    ("data/pages/bible/genesis/7.txt", include_str!("starter/bible/genesis/7.txt")),
    ("data/pages/bible/genesis/8.txt", include_str!("starter/bible/genesis/8.txt")),
    ("data/pages/bible/genesis/9.txt", include_str!("starter/bible/genesis/9.txt")),
    ("data/pages/bible/genesis/10.txt", include_str!("starter/bible/genesis/10.txt")),
    ("data/pages/bible/genesis/11.txt", include_str!("starter/bible/genesis/11.txt")),
    ("data/pages/bible/genesis/12.txt", include_str!("starter/bible/genesis/12.txt")),
    ("data/pages/bible/genesis/13.txt", include_str!("starter/bible/genesis/13.txt")),
    ("data/pages/bible/genesis/14.txt", include_str!("starter/bible/genesis/14.txt")),
    ("data/pages/bible/genesis/15.txt", include_str!("starter/bible/genesis/15.txt")),
    ("data/pages/bible/genesis/16.txt", include_str!("starter/bible/genesis/16.txt")),
    ("data/pages/bible/genesis/17.txt", include_str!("starter/bible/genesis/17.txt")),
    ("data/pages/bible/genesis/18.txt", include_str!("starter/bible/genesis/18.txt")),
    ("data/pages/bible/genesis/19.txt", include_str!("starter/bible/genesis/19.txt")),
    ("data/pages/bible/genesis/20.txt", include_str!("starter/bible/genesis/20.txt")),
    ("data/pages/bible/genesis/21.txt", include_str!("starter/bible/genesis/21.txt")),
    ("data/pages/bible/genesis/22.txt", include_str!("starter/bible/genesis/22.txt")),
    ("data/pages/bible/genesis/23.txt", include_str!("starter/bible/genesis/23.txt")),
    ("data/pages/bible/genesis/24.txt", include_str!("starter/bible/genesis/24.txt")),
    ("data/pages/bible/genesis/25.txt", include_str!("starter/bible/genesis/25.txt")),
    ("data/pages/bible/genesis/26.txt", include_str!("starter/bible/genesis/26.txt")),
    ("data/pages/bible/genesis/27.txt", include_str!("starter/bible/genesis/27.txt")),
    ("data/pages/bible/genesis/28.txt", include_str!("starter/bible/genesis/28.txt")),
    ("data/pages/bible/genesis/29.txt", include_str!("starter/bible/genesis/29.txt")),
    ("data/pages/bible/genesis/30.txt", include_str!("starter/bible/genesis/30.txt")),
    ("data/pages/bible/genesis/31.txt", include_str!("starter/bible/genesis/31.txt")),
    ("data/pages/bible/genesis/32.txt", include_str!("starter/bible/genesis/32.txt")),
    ("data/pages/bible/genesis/33.txt", include_str!("starter/bible/genesis/33.txt")),
    ("data/pages/bible/genesis/34.txt", include_str!("starter/bible/genesis/34.txt")),
    ("data/pages/bible/genesis/35.txt", include_str!("starter/bible/genesis/35.txt")),
    ("data/pages/bible/genesis/36.txt", include_str!("starter/bible/genesis/36.txt")),
    ("data/pages/bible/genesis/37.txt", include_str!("starter/bible/genesis/37.txt")),
    ("data/pages/bible/genesis/38.txt", include_str!("starter/bible/genesis/38.txt")),
    ("data/pages/bible/genesis/39.txt", include_str!("starter/bible/genesis/39.txt")),
    ("data/pages/bible/genesis/40.txt", include_str!("starter/bible/genesis/40.txt")),
    ("data/pages/bible/genesis/41.txt", include_str!("starter/bible/genesis/41.txt")),
    ("data/pages/bible/genesis/42.txt", include_str!("starter/bible/genesis/42.txt")),
    ("data/pages/bible/genesis/43.txt", include_str!("starter/bible/genesis/43.txt")),
    ("data/pages/bible/genesis/44.txt", include_str!("starter/bible/genesis/44.txt")),
    ("data/pages/bible/genesis/45.txt", include_str!("starter/bible/genesis/45.txt")),
    ("data/pages/bible/genesis/46.txt", include_str!("starter/bible/genesis/46.txt")),
    ("data/pages/bible/genesis/47.txt", include_str!("starter/bible/genesis/47.txt")),
    ("data/pages/bible/genesis/48.txt", include_str!("starter/bible/genesis/48.txt")),
    ("data/pages/bible/genesis/49.txt", include_str!("starter/bible/genesis/49.txt")),
    ("data/pages/bible/genesis/50.txt", include_str!("starter/bible/genesis/50.txt")),
    ("data/pages/bible/genesis/start.txt", include_str!("starter/bible/genesis/start.txt")),
    ("data/pages/bible/exodus/1.txt", include_str!("starter/bible/exodus/1.txt")),
    ("data/pages/bible/exodus/2.txt", include_str!("starter/bible/exodus/2.txt")),
    ("data/pages/bible/exodus/3.txt", include_str!("starter/bible/exodus/3.txt")),
    ("data/pages/bible/exodus/4.txt", include_str!("starter/bible/exodus/4.txt")),
    ("data/pages/bible/exodus/5.txt", include_str!("starter/bible/exodus/5.txt")),
    ("data/pages/bible/exodus/6.txt", include_str!("starter/bible/exodus/6.txt")),
    ("data/pages/bible/exodus/7.txt", include_str!("starter/bible/exodus/7.txt")),
    ("data/pages/bible/exodus/8.txt", include_str!("starter/bible/exodus/8.txt")),
    ("data/pages/bible/exodus/9.txt", include_str!("starter/bible/exodus/9.txt")),
    ("data/pages/bible/exodus/10.txt", include_str!("starter/bible/exodus/10.txt")),
    ("data/pages/bible/exodus/11.txt", include_str!("starter/bible/exodus/11.txt")),
    ("data/pages/bible/exodus/12.txt", include_str!("starter/bible/exodus/12.txt")),
    ("data/pages/bible/exodus/13.txt", include_str!("starter/bible/exodus/13.txt")),
    ("data/pages/bible/exodus/14.txt", include_str!("starter/bible/exodus/14.txt")),
    ("data/pages/bible/exodus/15.txt", include_str!("starter/bible/exodus/15.txt")),
    ("data/pages/bible/exodus/16.txt", include_str!("starter/bible/exodus/16.txt")),
    ("data/pages/bible/exodus/17.txt", include_str!("starter/bible/exodus/17.txt")),
    ("data/pages/bible/exodus/18.txt", include_str!("starter/bible/exodus/18.txt")),
    ("data/pages/bible/exodus/19.txt", include_str!("starter/bible/exodus/19.txt")),
    ("data/pages/bible/exodus/20.txt", include_str!("starter/bible/exodus/20.txt")),
    ("data/pages/bible/exodus/21.txt", include_str!("starter/bible/exodus/21.txt")),
    ("data/pages/bible/exodus/22.txt", include_str!("starter/bible/exodus/22.txt")),
    ("data/pages/bible/exodus/23.txt", include_str!("starter/bible/exodus/23.txt")),
    ("data/pages/bible/exodus/24.txt", include_str!("starter/bible/exodus/24.txt")),
    ("data/pages/bible/exodus/25.txt", include_str!("starter/bible/exodus/25.txt")),
    ("data/pages/bible/exodus/26.txt", include_str!("starter/bible/exodus/26.txt")),
    ("data/pages/bible/exodus/27.txt", include_str!("starter/bible/exodus/27.txt")),
    ("data/pages/bible/exodus/28.txt", include_str!("starter/bible/exodus/28.txt")),
    ("data/pages/bible/exodus/29.txt", include_str!("starter/bible/exodus/29.txt")),
    ("data/pages/bible/exodus/30.txt", include_str!("starter/bible/exodus/30.txt")),
    ("data/pages/bible/exodus/31.txt", include_str!("starter/bible/exodus/31.txt")),
    ("data/pages/bible/exodus/32.txt", include_str!("starter/bible/exodus/32.txt")),
    ("data/pages/bible/exodus/33.txt", include_str!("starter/bible/exodus/33.txt")),
    ("data/pages/bible/exodus/34.txt", include_str!("starter/bible/exodus/34.txt")),
    ("data/pages/bible/exodus/35.txt", include_str!("starter/bible/exodus/35.txt")),
    ("data/pages/bible/exodus/36.txt", include_str!("starter/bible/exodus/36.txt")),
    ("data/pages/bible/exodus/37.txt", include_str!("starter/bible/exodus/37.txt")),
    ("data/pages/bible/exodus/38.txt", include_str!("starter/bible/exodus/38.txt")),
    ("data/pages/bible/exodus/39.txt", include_str!("starter/bible/exodus/39.txt")),
    ("data/pages/bible/exodus/40.txt", include_str!("starter/bible/exodus/40.txt")),
    ("data/pages/bible/exodus/start.txt", include_str!("starter/bible/exodus/start.txt")),
    ("data/pages/bible/leviticus/1.txt", include_str!("starter/bible/leviticus/1.txt")),
    ("data/pages/bible/leviticus/2.txt", include_str!("starter/bible/leviticus/2.txt")),
    ("data/pages/bible/leviticus/3.txt", include_str!("starter/bible/leviticus/3.txt")),
    ("data/pages/bible/leviticus/4.txt", include_str!("starter/bible/leviticus/4.txt")),
    ("data/pages/bible/leviticus/5.txt", include_str!("starter/bible/leviticus/5.txt")),
    ("data/pages/bible/leviticus/6.txt", include_str!("starter/bible/leviticus/6.txt")),
    ("data/pages/bible/leviticus/7.txt", include_str!("starter/bible/leviticus/7.txt")),
    ("data/pages/bible/leviticus/8.txt", include_str!("starter/bible/leviticus/8.txt")),
    ("data/pages/bible/leviticus/9.txt", include_str!("starter/bible/leviticus/9.txt")),
    ("data/pages/bible/leviticus/10.txt", include_str!("starter/bible/leviticus/10.txt")),
    ("data/pages/bible/leviticus/11.txt", include_str!("starter/bible/leviticus/11.txt")),
    ("data/pages/bible/leviticus/12.txt", include_str!("starter/bible/leviticus/12.txt")),
    ("data/pages/bible/leviticus/13.txt", include_str!("starter/bible/leviticus/13.txt")),
    ("data/pages/bible/leviticus/14.txt", include_str!("starter/bible/leviticus/14.txt")),
    ("data/pages/bible/leviticus/15.txt", include_str!("starter/bible/leviticus/15.txt")),
    ("data/pages/bible/leviticus/16.txt", include_str!("starter/bible/leviticus/16.txt")),
    ("data/pages/bible/leviticus/17.txt", include_str!("starter/bible/leviticus/17.txt")),
    ("data/pages/bible/leviticus/18.txt", include_str!("starter/bible/leviticus/18.txt")),
    ("data/pages/bible/leviticus/19.txt", include_str!("starter/bible/leviticus/19.txt")),
    ("data/pages/bible/leviticus/20.txt", include_str!("starter/bible/leviticus/20.txt")),
    ("data/pages/bible/leviticus/21.txt", include_str!("starter/bible/leviticus/21.txt")),
    ("data/pages/bible/leviticus/22.txt", include_str!("starter/bible/leviticus/22.txt")),
    ("data/pages/bible/leviticus/23.txt", include_str!("starter/bible/leviticus/23.txt")),
    ("data/pages/bible/leviticus/24.txt", include_str!("starter/bible/leviticus/24.txt")),
    ("data/pages/bible/leviticus/25.txt", include_str!("starter/bible/leviticus/25.txt")),
    ("data/pages/bible/leviticus/26.txt", include_str!("starter/bible/leviticus/26.txt")),
    ("data/pages/bible/leviticus/27.txt", include_str!("starter/bible/leviticus/27.txt")),
    ("data/pages/bible/leviticus/start.txt", include_str!("starter/bible/leviticus/start.txt")),
    ("data/pages/bible/numbers/1.txt", include_str!("starter/bible/numbers/1.txt")),
    ("data/pages/bible/numbers/2.txt", include_str!("starter/bible/numbers/2.txt")),
    ("data/pages/bible/numbers/3.txt", include_str!("starter/bible/numbers/3.txt")),
    ("data/pages/bible/numbers/4.txt", include_str!("starter/bible/numbers/4.txt")),
    ("data/pages/bible/numbers/5.txt", include_str!("starter/bible/numbers/5.txt")),
    ("data/pages/bible/numbers/6.txt", include_str!("starter/bible/numbers/6.txt")),
    ("data/pages/bible/numbers/7.txt", include_str!("starter/bible/numbers/7.txt")),
    ("data/pages/bible/numbers/8.txt", include_str!("starter/bible/numbers/8.txt")),
    ("data/pages/bible/numbers/9.txt", include_str!("starter/bible/numbers/9.txt")),
    ("data/pages/bible/numbers/10.txt", include_str!("starter/bible/numbers/10.txt")),
    ("data/pages/bible/numbers/11.txt", include_str!("starter/bible/numbers/11.txt")),
    ("data/pages/bible/numbers/12.txt", include_str!("starter/bible/numbers/12.txt")),
    ("data/pages/bible/numbers/13.txt", include_str!("starter/bible/numbers/13.txt")),
    ("data/pages/bible/numbers/14.txt", include_str!("starter/bible/numbers/14.txt")),
    ("data/pages/bible/numbers/15.txt", include_str!("starter/bible/numbers/15.txt")),
    ("data/pages/bible/numbers/16.txt", include_str!("starter/bible/numbers/16.txt")),
    ("data/pages/bible/numbers/17.txt", include_str!("starter/bible/numbers/17.txt")),
    ("data/pages/bible/numbers/18.txt", include_str!("starter/bible/numbers/18.txt")),
    ("data/pages/bible/numbers/19.txt", include_str!("starter/bible/numbers/19.txt")),
    ("data/pages/bible/numbers/20.txt", include_str!("starter/bible/numbers/20.txt")),
    ("data/pages/bible/numbers/21.txt", include_str!("starter/bible/numbers/21.txt")),
    ("data/pages/bible/numbers/22.txt", include_str!("starter/bible/numbers/22.txt")),
    ("data/pages/bible/numbers/23.txt", include_str!("starter/bible/numbers/23.txt")),
    ("data/pages/bible/numbers/24.txt", include_str!("starter/bible/numbers/24.txt")),
    ("data/pages/bible/numbers/25.txt", include_str!("starter/bible/numbers/25.txt")),
    ("data/pages/bible/numbers/26.txt", include_str!("starter/bible/numbers/26.txt")),
    ("data/pages/bible/numbers/27.txt", include_str!("starter/bible/numbers/27.txt")),
    ("data/pages/bible/numbers/28.txt", include_str!("starter/bible/numbers/28.txt")),
    ("data/pages/bible/numbers/29.txt", include_str!("starter/bible/numbers/29.txt")),
    ("data/pages/bible/numbers/30.txt", include_str!("starter/bible/numbers/30.txt")),
    ("data/pages/bible/numbers/31.txt", include_str!("starter/bible/numbers/31.txt")),
    ("data/pages/bible/numbers/32.txt", include_str!("starter/bible/numbers/32.txt")),
    ("data/pages/bible/numbers/33.txt", include_str!("starter/bible/numbers/33.txt")),
    ("data/pages/bible/numbers/34.txt", include_str!("starter/bible/numbers/34.txt")),
    ("data/pages/bible/numbers/35.txt", include_str!("starter/bible/numbers/35.txt")),
    ("data/pages/bible/numbers/36.txt", include_str!("starter/bible/numbers/36.txt")),
    ("data/pages/bible/numbers/start.txt", include_str!("starter/bible/numbers/start.txt")),
    ("data/pages/bible/deuteronomy/1.txt", include_str!("starter/bible/deuteronomy/1.txt")),
    ("data/pages/bible/deuteronomy/2.txt", include_str!("starter/bible/deuteronomy/2.txt")),
    ("data/pages/bible/deuteronomy/3.txt", include_str!("starter/bible/deuteronomy/3.txt")),
    ("data/pages/bible/deuteronomy/4.txt", include_str!("starter/bible/deuteronomy/4.txt")),
    ("data/pages/bible/deuteronomy/5.txt", include_str!("starter/bible/deuteronomy/5.txt")),
    ("data/pages/bible/deuteronomy/6.txt", include_str!("starter/bible/deuteronomy/6.txt")),
    ("data/pages/bible/deuteronomy/7.txt", include_str!("starter/bible/deuteronomy/7.txt")),
    ("data/pages/bible/deuteronomy/8.txt", include_str!("starter/bible/deuteronomy/8.txt")),
    ("data/pages/bible/deuteronomy/9.txt", include_str!("starter/bible/deuteronomy/9.txt")),
    ("data/pages/bible/deuteronomy/10.txt", include_str!("starter/bible/deuteronomy/10.txt")),
    ("data/pages/bible/deuteronomy/11.txt", include_str!("starter/bible/deuteronomy/11.txt")),
    ("data/pages/bible/deuteronomy/12.txt", include_str!("starter/bible/deuteronomy/12.txt")),
    ("data/pages/bible/deuteronomy/13.txt", include_str!("starter/bible/deuteronomy/13.txt")),
    ("data/pages/bible/deuteronomy/14.txt", include_str!("starter/bible/deuteronomy/14.txt")),
    ("data/pages/bible/deuteronomy/15.txt", include_str!("starter/bible/deuteronomy/15.txt")),
    ("data/pages/bible/deuteronomy/16.txt", include_str!("starter/bible/deuteronomy/16.txt")),
    ("data/pages/bible/deuteronomy/17.txt", include_str!("starter/bible/deuteronomy/17.txt")),
    ("data/pages/bible/deuteronomy/18.txt", include_str!("starter/bible/deuteronomy/18.txt")),
    ("data/pages/bible/deuteronomy/19.txt", include_str!("starter/bible/deuteronomy/19.txt")),
    ("data/pages/bible/deuteronomy/20.txt", include_str!("starter/bible/deuteronomy/20.txt")),
    ("data/pages/bible/deuteronomy/21.txt", include_str!("starter/bible/deuteronomy/21.txt")),
    ("data/pages/bible/deuteronomy/22.txt", include_str!("starter/bible/deuteronomy/22.txt")),
    ("data/pages/bible/deuteronomy/23.txt", include_str!("starter/bible/deuteronomy/23.txt")),
    ("data/pages/bible/deuteronomy/24.txt", include_str!("starter/bible/deuteronomy/24.txt")),
    ("data/pages/bible/deuteronomy/25.txt", include_str!("starter/bible/deuteronomy/25.txt")),
    ("data/pages/bible/deuteronomy/26.txt", include_str!("starter/bible/deuteronomy/26.txt")),
    ("data/pages/bible/deuteronomy/27.txt", include_str!("starter/bible/deuteronomy/27.txt")),
    ("data/pages/bible/deuteronomy/28.txt", include_str!("starter/bible/deuteronomy/28.txt")),
    ("data/pages/bible/deuteronomy/29.txt", include_str!("starter/bible/deuteronomy/29.txt")),
    ("data/pages/bible/deuteronomy/30.txt", include_str!("starter/bible/deuteronomy/30.txt")),
    ("data/pages/bible/deuteronomy/31.txt", include_str!("starter/bible/deuteronomy/31.txt")),
    ("data/pages/bible/deuteronomy/32.txt", include_str!("starter/bible/deuteronomy/32.txt")),
    ("data/pages/bible/deuteronomy/33.txt", include_str!("starter/bible/deuteronomy/33.txt")),
    ("data/pages/bible/deuteronomy/34.txt", include_str!("starter/bible/deuteronomy/34.txt")),
    ("data/pages/bible/deuteronomy/start.txt", include_str!("starter/bible/deuteronomy/start.txt")),
    ("data/pages/bible/joshua/1.txt", include_str!("starter/bible/joshua/1.txt")),
    ("data/pages/bible/joshua/2.txt", include_str!("starter/bible/joshua/2.txt")),
    ("data/pages/bible/joshua/3.txt", include_str!("starter/bible/joshua/3.txt")),
    ("data/pages/bible/joshua/4.txt", include_str!("starter/bible/joshua/4.txt")),
    ("data/pages/bible/joshua/5.txt", include_str!("starter/bible/joshua/5.txt")),
    ("data/pages/bible/joshua/6.txt", include_str!("starter/bible/joshua/6.txt")),
    ("data/pages/bible/joshua/7.txt", include_str!("starter/bible/joshua/7.txt")),
    ("data/pages/bible/joshua/8.txt", include_str!("starter/bible/joshua/8.txt")),
    ("data/pages/bible/joshua/9.txt", include_str!("starter/bible/joshua/9.txt")),
    ("data/pages/bible/joshua/10.txt", include_str!("starter/bible/joshua/10.txt")),
    ("data/pages/bible/joshua/11.txt", include_str!("starter/bible/joshua/11.txt")),
    ("data/pages/bible/joshua/12.txt", include_str!("starter/bible/joshua/12.txt")),
    ("data/pages/bible/joshua/13.txt", include_str!("starter/bible/joshua/13.txt")),
    ("data/pages/bible/joshua/14.txt", include_str!("starter/bible/joshua/14.txt")),
    ("data/pages/bible/joshua/15.txt", include_str!("starter/bible/joshua/15.txt")),
    ("data/pages/bible/joshua/16.txt", include_str!("starter/bible/joshua/16.txt")),
    ("data/pages/bible/joshua/17.txt", include_str!("starter/bible/joshua/17.txt")),
    ("data/pages/bible/joshua/18.txt", include_str!("starter/bible/joshua/18.txt")),
    ("data/pages/bible/joshua/19.txt", include_str!("starter/bible/joshua/19.txt")),
    ("data/pages/bible/joshua/20.txt", include_str!("starter/bible/joshua/20.txt")),
    ("data/pages/bible/joshua/21.txt", include_str!("starter/bible/joshua/21.txt")),
    ("data/pages/bible/joshua/22.txt", include_str!("starter/bible/joshua/22.txt")),
    ("data/pages/bible/joshua/23.txt", include_str!("starter/bible/joshua/23.txt")),
    ("data/pages/bible/joshua/24.txt", include_str!("starter/bible/joshua/24.txt")),
    ("data/pages/bible/joshua/start.txt", include_str!("starter/bible/joshua/start.txt")),
    ("data/pages/bible/judges/1.txt", include_str!("starter/bible/judges/1.txt")),
    ("data/pages/bible/judges/2.txt", include_str!("starter/bible/judges/2.txt")),
    ("data/pages/bible/judges/3.txt", include_str!("starter/bible/judges/3.txt")),
    ("data/pages/bible/judges/4.txt", include_str!("starter/bible/judges/4.txt")),
    ("data/pages/bible/judges/5.txt", include_str!("starter/bible/judges/5.txt")),
    ("data/pages/bible/judges/6.txt", include_str!("starter/bible/judges/6.txt")),
    ("data/pages/bible/judges/7.txt", include_str!("starter/bible/judges/7.txt")),
    ("data/pages/bible/judges/8.txt", include_str!("starter/bible/judges/8.txt")),
    ("data/pages/bible/judges/9.txt", include_str!("starter/bible/judges/9.txt")),
    ("data/pages/bible/judges/10.txt", include_str!("starter/bible/judges/10.txt")),
    ("data/pages/bible/judges/11.txt", include_str!("starter/bible/judges/11.txt")),
    ("data/pages/bible/judges/12.txt", include_str!("starter/bible/judges/12.txt")),
    ("data/pages/bible/judges/13.txt", include_str!("starter/bible/judges/13.txt")),
    ("data/pages/bible/judges/14.txt", include_str!("starter/bible/judges/14.txt")),
    ("data/pages/bible/judges/15.txt", include_str!("starter/bible/judges/15.txt")),
    ("data/pages/bible/judges/16.txt", include_str!("starter/bible/judges/16.txt")),
    ("data/pages/bible/judges/17.txt", include_str!("starter/bible/judges/17.txt")),
    ("data/pages/bible/judges/18.txt", include_str!("starter/bible/judges/18.txt")),
    ("data/pages/bible/judges/19.txt", include_str!("starter/bible/judges/19.txt")),
    ("data/pages/bible/judges/20.txt", include_str!("starter/bible/judges/20.txt")),
    ("data/pages/bible/judges/21.txt", include_str!("starter/bible/judges/21.txt")),
    ("data/pages/bible/judges/start.txt", include_str!("starter/bible/judges/start.txt")),
    ("data/pages/bible/ruth/1.txt", include_str!("starter/bible/ruth/1.txt")),
    ("data/pages/bible/ruth/2.txt", include_str!("starter/bible/ruth/2.txt")),
    ("data/pages/bible/ruth/3.txt", include_str!("starter/bible/ruth/3.txt")),
    ("data/pages/bible/ruth/4.txt", include_str!("starter/bible/ruth/4.txt")),
    ("data/pages/bible/ruth/start.txt", include_str!("starter/bible/ruth/start.txt")),
    ("data/pages/bible/1_samuel/1.txt", include_str!("starter/bible/1_samuel/1.txt")),
    ("data/pages/bible/1_samuel/2.txt", include_str!("starter/bible/1_samuel/2.txt")),
    ("data/pages/bible/1_samuel/3.txt", include_str!("starter/bible/1_samuel/3.txt")),
    ("data/pages/bible/1_samuel/4.txt", include_str!("starter/bible/1_samuel/4.txt")),
    ("data/pages/bible/1_samuel/5.txt", include_str!("starter/bible/1_samuel/5.txt")),
    ("data/pages/bible/1_samuel/6.txt", include_str!("starter/bible/1_samuel/6.txt")),
    ("data/pages/bible/1_samuel/7.txt", include_str!("starter/bible/1_samuel/7.txt")),
    ("data/pages/bible/1_samuel/8.txt", include_str!("starter/bible/1_samuel/8.txt")),
    ("data/pages/bible/1_samuel/9.txt", include_str!("starter/bible/1_samuel/9.txt")),
    ("data/pages/bible/1_samuel/10.txt", include_str!("starter/bible/1_samuel/10.txt")),
    ("data/pages/bible/1_samuel/11.txt", include_str!("starter/bible/1_samuel/11.txt")),
    ("data/pages/bible/1_samuel/12.txt", include_str!("starter/bible/1_samuel/12.txt")),
    ("data/pages/bible/1_samuel/13.txt", include_str!("starter/bible/1_samuel/13.txt")),
    ("data/pages/bible/1_samuel/14.txt", include_str!("starter/bible/1_samuel/14.txt")),
    ("data/pages/bible/1_samuel/15.txt", include_str!("starter/bible/1_samuel/15.txt")),
    ("data/pages/bible/1_samuel/16.txt", include_str!("starter/bible/1_samuel/16.txt")),
    ("data/pages/bible/1_samuel/17.txt", include_str!("starter/bible/1_samuel/17.txt")),
    ("data/pages/bible/1_samuel/18.txt", include_str!("starter/bible/1_samuel/18.txt")),
    ("data/pages/bible/1_samuel/19.txt", include_str!("starter/bible/1_samuel/19.txt")),
    ("data/pages/bible/1_samuel/20.txt", include_str!("starter/bible/1_samuel/20.txt")),
    ("data/pages/bible/1_samuel/21.txt", include_str!("starter/bible/1_samuel/21.txt")),
    ("data/pages/bible/1_samuel/22.txt", include_str!("starter/bible/1_samuel/22.txt")),
    ("data/pages/bible/1_samuel/23.txt", include_str!("starter/bible/1_samuel/23.txt")),
    ("data/pages/bible/1_samuel/24.txt", include_str!("starter/bible/1_samuel/24.txt")),
    ("data/pages/bible/1_samuel/25.txt", include_str!("starter/bible/1_samuel/25.txt")),
    ("data/pages/bible/1_samuel/26.txt", include_str!("starter/bible/1_samuel/26.txt")),
    ("data/pages/bible/1_samuel/27.txt", include_str!("starter/bible/1_samuel/27.txt")),
    ("data/pages/bible/1_samuel/28.txt", include_str!("starter/bible/1_samuel/28.txt")),
    ("data/pages/bible/1_samuel/29.txt", include_str!("starter/bible/1_samuel/29.txt")),
    ("data/pages/bible/1_samuel/30.txt", include_str!("starter/bible/1_samuel/30.txt")),
    ("data/pages/bible/1_samuel/31.txt", include_str!("starter/bible/1_samuel/31.txt")),
    ("data/pages/bible/1_samuel/start.txt", include_str!("starter/bible/1_samuel/start.txt")),
    ("data/pages/bible/2_samuel/1.txt", include_str!("starter/bible/2_samuel/1.txt")),
    ("data/pages/bible/2_samuel/2.txt", include_str!("starter/bible/2_samuel/2.txt")),
    ("data/pages/bible/2_samuel/3.txt", include_str!("starter/bible/2_samuel/3.txt")),
    ("data/pages/bible/2_samuel/4.txt", include_str!("starter/bible/2_samuel/4.txt")),
    ("data/pages/bible/2_samuel/5.txt", include_str!("starter/bible/2_samuel/5.txt")),
    ("data/pages/bible/2_samuel/6.txt", include_str!("starter/bible/2_samuel/6.txt")),
    ("data/pages/bible/2_samuel/7.txt", include_str!("starter/bible/2_samuel/7.txt")),
    ("data/pages/bible/2_samuel/8.txt", include_str!("starter/bible/2_samuel/8.txt")),
    ("data/pages/bible/2_samuel/9.txt", include_str!("starter/bible/2_samuel/9.txt")),
    ("data/pages/bible/2_samuel/10.txt", include_str!("starter/bible/2_samuel/10.txt")),
    ("data/pages/bible/2_samuel/11.txt", include_str!("starter/bible/2_samuel/11.txt")),
    ("data/pages/bible/2_samuel/12.txt", include_str!("starter/bible/2_samuel/12.txt")),
    ("data/pages/bible/2_samuel/13.txt", include_str!("starter/bible/2_samuel/13.txt")),
    ("data/pages/bible/2_samuel/14.txt", include_str!("starter/bible/2_samuel/14.txt")),
    ("data/pages/bible/2_samuel/15.txt", include_str!("starter/bible/2_samuel/15.txt")),
    ("data/pages/bible/2_samuel/16.txt", include_str!("starter/bible/2_samuel/16.txt")),
    ("data/pages/bible/2_samuel/17.txt", include_str!("starter/bible/2_samuel/17.txt")),
    ("data/pages/bible/2_samuel/18.txt", include_str!("starter/bible/2_samuel/18.txt")),
    ("data/pages/bible/2_samuel/19.txt", include_str!("starter/bible/2_samuel/19.txt")),
    ("data/pages/bible/2_samuel/20.txt", include_str!("starter/bible/2_samuel/20.txt")),
    ("data/pages/bible/2_samuel/21.txt", include_str!("starter/bible/2_samuel/21.txt")),
    ("data/pages/bible/2_samuel/22.txt", include_str!("starter/bible/2_samuel/22.txt")),
    ("data/pages/bible/2_samuel/23.txt", include_str!("starter/bible/2_samuel/23.txt")),
    ("data/pages/bible/2_samuel/24.txt", include_str!("starter/bible/2_samuel/24.txt")),
    ("data/pages/bible/2_samuel/start.txt", include_str!("starter/bible/2_samuel/start.txt")),
    ("data/pages/bible/1_kings/1.txt", include_str!("starter/bible/1_kings/1.txt")),
    ("data/pages/bible/1_kings/2.txt", include_str!("starter/bible/1_kings/2.txt")),
    ("data/pages/bible/1_kings/3.txt", include_str!("starter/bible/1_kings/3.txt")),
    ("data/pages/bible/1_kings/4.txt", include_str!("starter/bible/1_kings/4.txt")),
    ("data/pages/bible/1_kings/5.txt", include_str!("starter/bible/1_kings/5.txt")),
    ("data/pages/bible/1_kings/6.txt", include_str!("starter/bible/1_kings/6.txt")),
    ("data/pages/bible/1_kings/7.txt", include_str!("starter/bible/1_kings/7.txt")),
    ("data/pages/bible/1_kings/8.txt", include_str!("starter/bible/1_kings/8.txt")),
    ("data/pages/bible/1_kings/9.txt", include_str!("starter/bible/1_kings/9.txt")),
    ("data/pages/bible/1_kings/10.txt", include_str!("starter/bible/1_kings/10.txt")),
    ("data/pages/bible/1_kings/11.txt", include_str!("starter/bible/1_kings/11.txt")),
    ("data/pages/bible/1_kings/12.txt", include_str!("starter/bible/1_kings/12.txt")),
    ("data/pages/bible/1_kings/13.txt", include_str!("starter/bible/1_kings/13.txt")),
    ("data/pages/bible/1_kings/14.txt", include_str!("starter/bible/1_kings/14.txt")),
    ("data/pages/bible/1_kings/15.txt", include_str!("starter/bible/1_kings/15.txt")),
    ("data/pages/bible/1_kings/16.txt", include_str!("starter/bible/1_kings/16.txt")),
    ("data/pages/bible/1_kings/17.txt", include_str!("starter/bible/1_kings/17.txt")),
    ("data/pages/bible/1_kings/18.txt", include_str!("starter/bible/1_kings/18.txt")),
    ("data/pages/bible/1_kings/19.txt", include_str!("starter/bible/1_kings/19.txt")),
    ("data/pages/bible/1_kings/20.txt", include_str!("starter/bible/1_kings/20.txt")),
    ("data/pages/bible/1_kings/21.txt", include_str!("starter/bible/1_kings/21.txt")),
    ("data/pages/bible/1_kings/22.txt", include_str!("starter/bible/1_kings/22.txt")),
    ("data/pages/bible/1_kings/start.txt", include_str!("starter/bible/1_kings/start.txt")),
    ("data/pages/bible/2_kings/1.txt", include_str!("starter/bible/2_kings/1.txt")),
    ("data/pages/bible/2_kings/2.txt", include_str!("starter/bible/2_kings/2.txt")),
    ("data/pages/bible/2_kings/3.txt", include_str!("starter/bible/2_kings/3.txt")),
    ("data/pages/bible/2_kings/4.txt", include_str!("starter/bible/2_kings/4.txt")),
    ("data/pages/bible/2_kings/5.txt", include_str!("starter/bible/2_kings/5.txt")),
    ("data/pages/bible/2_kings/6.txt", include_str!("starter/bible/2_kings/6.txt")),
    ("data/pages/bible/2_kings/7.txt", include_str!("starter/bible/2_kings/7.txt")),
    ("data/pages/bible/2_kings/8.txt", include_str!("starter/bible/2_kings/8.txt")),
    ("data/pages/bible/2_kings/9.txt", include_str!("starter/bible/2_kings/9.txt")),
    ("data/pages/bible/2_kings/10.txt", include_str!("starter/bible/2_kings/10.txt")),
    ("data/pages/bible/2_kings/11.txt", include_str!("starter/bible/2_kings/11.txt")),
    ("data/pages/bible/2_kings/12.txt", include_str!("starter/bible/2_kings/12.txt")),
    ("data/pages/bible/2_kings/13.txt", include_str!("starter/bible/2_kings/13.txt")),
    ("data/pages/bible/2_kings/14.txt", include_str!("starter/bible/2_kings/14.txt")),
    ("data/pages/bible/2_kings/15.txt", include_str!("starter/bible/2_kings/15.txt")),
    ("data/pages/bible/2_kings/16.txt", include_str!("starter/bible/2_kings/16.txt")),
    ("data/pages/bible/2_kings/17.txt", include_str!("starter/bible/2_kings/17.txt")),
    ("data/pages/bible/2_kings/18.txt", include_str!("starter/bible/2_kings/18.txt")),
    ("data/pages/bible/2_kings/19.txt", include_str!("starter/bible/2_kings/19.txt")),
    ("data/pages/bible/2_kings/20.txt", include_str!("starter/bible/2_kings/20.txt")),
    ("data/pages/bible/2_kings/21.txt", include_str!("starter/bible/2_kings/21.txt")),
    ("data/pages/bible/2_kings/22.txt", include_str!("starter/bible/2_kings/22.txt")),
    ("data/pages/bible/2_kings/23.txt", include_str!("starter/bible/2_kings/23.txt")),
    ("data/pages/bible/2_kings/24.txt", include_str!("starter/bible/2_kings/24.txt")),
    ("data/pages/bible/2_kings/25.txt", include_str!("starter/bible/2_kings/25.txt")),
    ("data/pages/bible/2_kings/start.txt", include_str!("starter/bible/2_kings/start.txt")),
    ("data/pages/bible/1_chronicles/1.txt", include_str!("starter/bible/1_chronicles/1.txt")),
    ("data/pages/bible/1_chronicles/2.txt", include_str!("starter/bible/1_chronicles/2.txt")),
    ("data/pages/bible/1_chronicles/3.txt", include_str!("starter/bible/1_chronicles/3.txt")),
    ("data/pages/bible/1_chronicles/4.txt", include_str!("starter/bible/1_chronicles/4.txt")),
    ("data/pages/bible/1_chronicles/5.txt", include_str!("starter/bible/1_chronicles/5.txt")),
    ("data/pages/bible/1_chronicles/6.txt", include_str!("starter/bible/1_chronicles/6.txt")),
    ("data/pages/bible/1_chronicles/7.txt", include_str!("starter/bible/1_chronicles/7.txt")),
    ("data/pages/bible/1_chronicles/8.txt", include_str!("starter/bible/1_chronicles/8.txt")),
    ("data/pages/bible/1_chronicles/9.txt", include_str!("starter/bible/1_chronicles/9.txt")),
    ("data/pages/bible/1_chronicles/10.txt", include_str!("starter/bible/1_chronicles/10.txt")),
    ("data/pages/bible/1_chronicles/11.txt", include_str!("starter/bible/1_chronicles/11.txt")),
    ("data/pages/bible/1_chronicles/12.txt", include_str!("starter/bible/1_chronicles/12.txt")),
    ("data/pages/bible/1_chronicles/13.txt", include_str!("starter/bible/1_chronicles/13.txt")),
    ("data/pages/bible/1_chronicles/14.txt", include_str!("starter/bible/1_chronicles/14.txt")),
    ("data/pages/bible/1_chronicles/15.txt", include_str!("starter/bible/1_chronicles/15.txt")),
    ("data/pages/bible/1_chronicles/16.txt", include_str!("starter/bible/1_chronicles/16.txt")),
    ("data/pages/bible/1_chronicles/17.txt", include_str!("starter/bible/1_chronicles/17.txt")),
    ("data/pages/bible/1_chronicles/18.txt", include_str!("starter/bible/1_chronicles/18.txt")),
    ("data/pages/bible/1_chronicles/19.txt", include_str!("starter/bible/1_chronicles/19.txt")),
    ("data/pages/bible/1_chronicles/20.txt", include_str!("starter/bible/1_chronicles/20.txt")),
    ("data/pages/bible/1_chronicles/21.txt", include_str!("starter/bible/1_chronicles/21.txt")),
    ("data/pages/bible/1_chronicles/22.txt", include_str!("starter/bible/1_chronicles/22.txt")),
    ("data/pages/bible/1_chronicles/23.txt", include_str!("starter/bible/1_chronicles/23.txt")),
    ("data/pages/bible/1_chronicles/24.txt", include_str!("starter/bible/1_chronicles/24.txt")),
    ("data/pages/bible/1_chronicles/25.txt", include_str!("starter/bible/1_chronicles/25.txt")),
    ("data/pages/bible/1_chronicles/26.txt", include_str!("starter/bible/1_chronicles/26.txt")),
    ("data/pages/bible/1_chronicles/27.txt", include_str!("starter/bible/1_chronicles/27.txt")),
    ("data/pages/bible/1_chronicles/28.txt", include_str!("starter/bible/1_chronicles/28.txt")),
    ("data/pages/bible/1_chronicles/29.txt", include_str!("starter/bible/1_chronicles/29.txt")),
    ("data/pages/bible/1_chronicles/start.txt", include_str!("starter/bible/1_chronicles/start.txt")),
    ("data/pages/bible/2_chronicles/1.txt", include_str!("starter/bible/2_chronicles/1.txt")),
    ("data/pages/bible/2_chronicles/2.txt", include_str!("starter/bible/2_chronicles/2.txt")),
    ("data/pages/bible/2_chronicles/3.txt", include_str!("starter/bible/2_chronicles/3.txt")),
    ("data/pages/bible/2_chronicles/4.txt", include_str!("starter/bible/2_chronicles/4.txt")),
    ("data/pages/bible/2_chronicles/5.txt", include_str!("starter/bible/2_chronicles/5.txt")),
    ("data/pages/bible/2_chronicles/6.txt", include_str!("starter/bible/2_chronicles/6.txt")),
    ("data/pages/bible/2_chronicles/7.txt", include_str!("starter/bible/2_chronicles/7.txt")),
    ("data/pages/bible/2_chronicles/8.txt", include_str!("starter/bible/2_chronicles/8.txt")),
    ("data/pages/bible/2_chronicles/9.txt", include_str!("starter/bible/2_chronicles/9.txt")),
    ("data/pages/bible/2_chronicles/10.txt", include_str!("starter/bible/2_chronicles/10.txt")),
    ("data/pages/bible/2_chronicles/11.txt", include_str!("starter/bible/2_chronicles/11.txt")),
    ("data/pages/bible/2_chronicles/12.txt", include_str!("starter/bible/2_chronicles/12.txt")),
    ("data/pages/bible/2_chronicles/13.txt", include_str!("starter/bible/2_chronicles/13.txt")),
    ("data/pages/bible/2_chronicles/14.txt", include_str!("starter/bible/2_chronicles/14.txt")),
    ("data/pages/bible/2_chronicles/15.txt", include_str!("starter/bible/2_chronicles/15.txt")),
    ("data/pages/bible/2_chronicles/16.txt", include_str!("starter/bible/2_chronicles/16.txt")),
    ("data/pages/bible/2_chronicles/17.txt", include_str!("starter/bible/2_chronicles/17.txt")),
    ("data/pages/bible/2_chronicles/18.txt", include_str!("starter/bible/2_chronicles/18.txt")),
    ("data/pages/bible/2_chronicles/19.txt", include_str!("starter/bible/2_chronicles/19.txt")),
    ("data/pages/bible/2_chronicles/20.txt", include_str!("starter/bible/2_chronicles/20.txt")),
    ("data/pages/bible/2_chronicles/21.txt", include_str!("starter/bible/2_chronicles/21.txt")),
    ("data/pages/bible/2_chronicles/22.txt", include_str!("starter/bible/2_chronicles/22.txt")),
    ("data/pages/bible/2_chronicles/23.txt", include_str!("starter/bible/2_chronicles/23.txt")),
    ("data/pages/bible/2_chronicles/24.txt", include_str!("starter/bible/2_chronicles/24.txt")),
    ("data/pages/bible/2_chronicles/25.txt", include_str!("starter/bible/2_chronicles/25.txt")),
    ("data/pages/bible/2_chronicles/26.txt", include_str!("starter/bible/2_chronicles/26.txt")),
    ("data/pages/bible/2_chronicles/27.txt", include_str!("starter/bible/2_chronicles/27.txt")),
    ("data/pages/bible/2_chronicles/28.txt", include_str!("starter/bible/2_chronicles/28.txt")),
    ("data/pages/bible/2_chronicles/29.txt", include_str!("starter/bible/2_chronicles/29.txt")),
    ("data/pages/bible/2_chronicles/30.txt", include_str!("starter/bible/2_chronicles/30.txt")),
    ("data/pages/bible/2_chronicles/31.txt", include_str!("starter/bible/2_chronicles/31.txt")),
    ("data/pages/bible/2_chronicles/32.txt", include_str!("starter/bible/2_chronicles/32.txt")),
    ("data/pages/bible/2_chronicles/33.txt", include_str!("starter/bible/2_chronicles/33.txt")),
    ("data/pages/bible/2_chronicles/34.txt", include_str!("starter/bible/2_chronicles/34.txt")),
    ("data/pages/bible/2_chronicles/35.txt", include_str!("starter/bible/2_chronicles/35.txt")),
    ("data/pages/bible/2_chronicles/36.txt", include_str!("starter/bible/2_chronicles/36.txt")),
    ("data/pages/bible/2_chronicles/start.txt", include_str!("starter/bible/2_chronicles/start.txt")),
    ("data/pages/bible/ezra/1.txt", include_str!("starter/bible/ezra/1.txt")),
    ("data/pages/bible/ezra/2.txt", include_str!("starter/bible/ezra/2.txt")),
    ("data/pages/bible/ezra/3.txt", include_str!("starter/bible/ezra/3.txt")),
    ("data/pages/bible/ezra/4.txt", include_str!("starter/bible/ezra/4.txt")),
    ("data/pages/bible/ezra/5.txt", include_str!("starter/bible/ezra/5.txt")),
    ("data/pages/bible/ezra/6.txt", include_str!("starter/bible/ezra/6.txt")),
    ("data/pages/bible/ezra/7.txt", include_str!("starter/bible/ezra/7.txt")),
    ("data/pages/bible/ezra/8.txt", include_str!("starter/bible/ezra/8.txt")),
    ("data/pages/bible/ezra/9.txt", include_str!("starter/bible/ezra/9.txt")),
    ("data/pages/bible/ezra/10.txt", include_str!("starter/bible/ezra/10.txt")),
    ("data/pages/bible/ezra/start.txt", include_str!("starter/bible/ezra/start.txt")),
    ("data/pages/bible/nehemiah/1.txt", include_str!("starter/bible/nehemiah/1.txt")),
    ("data/pages/bible/nehemiah/2.txt", include_str!("starter/bible/nehemiah/2.txt")),
    ("data/pages/bible/nehemiah/3.txt", include_str!("starter/bible/nehemiah/3.txt")),
    ("data/pages/bible/nehemiah/4.txt", include_str!("starter/bible/nehemiah/4.txt")),
    ("data/pages/bible/nehemiah/5.txt", include_str!("starter/bible/nehemiah/5.txt")),
    ("data/pages/bible/nehemiah/6.txt", include_str!("starter/bible/nehemiah/6.txt")),
    ("data/pages/bible/nehemiah/7.txt", include_str!("starter/bible/nehemiah/7.txt")),
    ("data/pages/bible/nehemiah/8.txt", include_str!("starter/bible/nehemiah/8.txt")),
    ("data/pages/bible/nehemiah/9.txt", include_str!("starter/bible/nehemiah/9.txt")),
    ("data/pages/bible/nehemiah/10.txt", include_str!("starter/bible/nehemiah/10.txt")),
    ("data/pages/bible/nehemiah/11.txt", include_str!("starter/bible/nehemiah/11.txt")),
    ("data/pages/bible/nehemiah/12.txt", include_str!("starter/bible/nehemiah/12.txt")),
    ("data/pages/bible/nehemiah/13.txt", include_str!("starter/bible/nehemiah/13.txt")),
    ("data/pages/bible/nehemiah/start.txt", include_str!("starter/bible/nehemiah/start.txt")),
    ("data/pages/bible/esther/1.txt", include_str!("starter/bible/esther/1.txt")),
    ("data/pages/bible/esther/2.txt", include_str!("starter/bible/esther/2.txt")),
    ("data/pages/bible/esther/3.txt", include_str!("starter/bible/esther/3.txt")),
    ("data/pages/bible/esther/4.txt", include_str!("starter/bible/esther/4.txt")),
    ("data/pages/bible/esther/5.txt", include_str!("starter/bible/esther/5.txt")),
    ("data/pages/bible/esther/6.txt", include_str!("starter/bible/esther/6.txt")),
    ("data/pages/bible/esther/7.txt", include_str!("starter/bible/esther/7.txt")),
    ("data/pages/bible/esther/8.txt", include_str!("starter/bible/esther/8.txt")),
    ("data/pages/bible/esther/9.txt", include_str!("starter/bible/esther/9.txt")),
    ("data/pages/bible/esther/10.txt", include_str!("starter/bible/esther/10.txt")),
    ("data/pages/bible/esther/start.txt", include_str!("starter/bible/esther/start.txt")),
    ("data/pages/bible/job/1.txt", include_str!("starter/bible/job/1.txt")),
    ("data/pages/bible/job/2.txt", include_str!("starter/bible/job/2.txt")),
    ("data/pages/bible/job/3.txt", include_str!("starter/bible/job/3.txt")),
    ("data/pages/bible/job/4.txt", include_str!("starter/bible/job/4.txt")),
    ("data/pages/bible/job/5.txt", include_str!("starter/bible/job/5.txt")),
    ("data/pages/bible/job/6.txt", include_str!("starter/bible/job/6.txt")),
    ("data/pages/bible/job/7.txt", include_str!("starter/bible/job/7.txt")),
    ("data/pages/bible/job/8.txt", include_str!("starter/bible/job/8.txt")),
    ("data/pages/bible/job/9.txt", include_str!("starter/bible/job/9.txt")),
    ("data/pages/bible/job/10.txt", include_str!("starter/bible/job/10.txt")),
    ("data/pages/bible/job/11.txt", include_str!("starter/bible/job/11.txt")),
    ("data/pages/bible/job/12.txt", include_str!("starter/bible/job/12.txt")),
    ("data/pages/bible/job/13.txt", include_str!("starter/bible/job/13.txt")),
    ("data/pages/bible/job/14.txt", include_str!("starter/bible/job/14.txt")),
    ("data/pages/bible/job/15.txt", include_str!("starter/bible/job/15.txt")),
    ("data/pages/bible/job/16.txt", include_str!("starter/bible/job/16.txt")),
    ("data/pages/bible/job/17.txt", include_str!("starter/bible/job/17.txt")),
    ("data/pages/bible/job/18.txt", include_str!("starter/bible/job/18.txt")),
    ("data/pages/bible/job/19.txt", include_str!("starter/bible/job/19.txt")),
    ("data/pages/bible/job/20.txt", include_str!("starter/bible/job/20.txt")),
    ("data/pages/bible/job/21.txt", include_str!("starter/bible/job/21.txt")),
    ("data/pages/bible/job/22.txt", include_str!("starter/bible/job/22.txt")),
    ("data/pages/bible/job/23.txt", include_str!("starter/bible/job/23.txt")),
    ("data/pages/bible/job/24.txt", include_str!("starter/bible/job/24.txt")),
    ("data/pages/bible/job/25.txt", include_str!("starter/bible/job/25.txt")),
    ("data/pages/bible/job/26.txt", include_str!("starter/bible/job/26.txt")),
    ("data/pages/bible/job/27.txt", include_str!("starter/bible/job/27.txt")),
    ("data/pages/bible/job/28.txt", include_str!("starter/bible/job/28.txt")),
    ("data/pages/bible/job/29.txt", include_str!("starter/bible/job/29.txt")),
    ("data/pages/bible/job/30.txt", include_str!("starter/bible/job/30.txt")),
    ("data/pages/bible/job/31.txt", include_str!("starter/bible/job/31.txt")),
    ("data/pages/bible/job/32.txt", include_str!("starter/bible/job/32.txt")),
    ("data/pages/bible/job/33.txt", include_str!("starter/bible/job/33.txt")),
    ("data/pages/bible/job/34.txt", include_str!("starter/bible/job/34.txt")),
    ("data/pages/bible/job/35.txt", include_str!("starter/bible/job/35.txt")),
    ("data/pages/bible/job/36.txt", include_str!("starter/bible/job/36.txt")),
    ("data/pages/bible/job/37.txt", include_str!("starter/bible/job/37.txt")),
    ("data/pages/bible/job/38.txt", include_str!("starter/bible/job/38.txt")),
    ("data/pages/bible/job/39.txt", include_str!("starter/bible/job/39.txt")),
    ("data/pages/bible/job/40.txt", include_str!("starter/bible/job/40.txt")),
    ("data/pages/bible/job/41.txt", include_str!("starter/bible/job/41.txt")),
    ("data/pages/bible/job/42.txt", include_str!("starter/bible/job/42.txt")),
    ("data/pages/bible/job/start.txt", include_str!("starter/bible/job/start.txt")),
    ("data/pages/bible/psalms/1.txt", include_str!("starter/bible/psalms/1.txt")),
    ("data/pages/bible/psalms/2.txt", include_str!("starter/bible/psalms/2.txt")),
    ("data/pages/bible/psalms/3.txt", include_str!("starter/bible/psalms/3.txt")),
    ("data/pages/bible/psalms/4.txt", include_str!("starter/bible/psalms/4.txt")),
    ("data/pages/bible/psalms/5.txt", include_str!("starter/bible/psalms/5.txt")),
    ("data/pages/bible/psalms/6.txt", include_str!("starter/bible/psalms/6.txt")),
    ("data/pages/bible/psalms/7.txt", include_str!("starter/bible/psalms/7.txt")),
    ("data/pages/bible/psalms/8.txt", include_str!("starter/bible/psalms/8.txt")),
    ("data/pages/bible/psalms/9.txt", include_str!("starter/bible/psalms/9.txt")),
    ("data/pages/bible/psalms/10.txt", include_str!("starter/bible/psalms/10.txt")),
    ("data/pages/bible/psalms/11.txt", include_str!("starter/bible/psalms/11.txt")),
    ("data/pages/bible/psalms/12.txt", include_str!("starter/bible/psalms/12.txt")),
    ("data/pages/bible/psalms/13.txt", include_str!("starter/bible/psalms/13.txt")),
    ("data/pages/bible/psalms/14.txt", include_str!("starter/bible/psalms/14.txt")),
    ("data/pages/bible/psalms/15.txt", include_str!("starter/bible/psalms/15.txt")),
    ("data/pages/bible/psalms/16.txt", include_str!("starter/bible/psalms/16.txt")),
    ("data/pages/bible/psalms/17.txt", include_str!("starter/bible/psalms/17.txt")),
    ("data/pages/bible/psalms/18.txt", include_str!("starter/bible/psalms/18.txt")),
    ("data/pages/bible/psalms/19.txt", include_str!("starter/bible/psalms/19.txt")),
    ("data/pages/bible/psalms/20.txt", include_str!("starter/bible/psalms/20.txt")),
    ("data/pages/bible/psalms/21.txt", include_str!("starter/bible/psalms/21.txt")),
    ("data/pages/bible/psalms/22.txt", include_str!("starter/bible/psalms/22.txt")),
    ("data/pages/bible/psalms/23.txt", include_str!("starter/bible/psalms/23.txt")),
    ("data/pages/bible/psalms/24.txt", include_str!("starter/bible/psalms/24.txt")),
    ("data/pages/bible/psalms/25.txt", include_str!("starter/bible/psalms/25.txt")),
    ("data/pages/bible/psalms/26.txt", include_str!("starter/bible/psalms/26.txt")),
    ("data/pages/bible/psalms/27.txt", include_str!("starter/bible/psalms/27.txt")),
    ("data/pages/bible/psalms/28.txt", include_str!("starter/bible/psalms/28.txt")),
    ("data/pages/bible/psalms/29.txt", include_str!("starter/bible/psalms/29.txt")),
    ("data/pages/bible/psalms/30.txt", include_str!("starter/bible/psalms/30.txt")),
    ("data/pages/bible/psalms/31.txt", include_str!("starter/bible/psalms/31.txt")),
    ("data/pages/bible/psalms/32.txt", include_str!("starter/bible/psalms/32.txt")),
    ("data/pages/bible/psalms/33.txt", include_str!("starter/bible/psalms/33.txt")),
    ("data/pages/bible/psalms/34.txt", include_str!("starter/bible/psalms/34.txt")),
    ("data/pages/bible/psalms/35.txt", include_str!("starter/bible/psalms/35.txt")),
    ("data/pages/bible/psalms/36.txt", include_str!("starter/bible/psalms/36.txt")),
    ("data/pages/bible/psalms/37.txt", include_str!("starter/bible/psalms/37.txt")),
    ("data/pages/bible/psalms/38.txt", include_str!("starter/bible/psalms/38.txt")),
    ("data/pages/bible/psalms/39.txt", include_str!("starter/bible/psalms/39.txt")),
    ("data/pages/bible/psalms/40.txt", include_str!("starter/bible/psalms/40.txt")),
    ("data/pages/bible/psalms/41.txt", include_str!("starter/bible/psalms/41.txt")),
    ("data/pages/bible/psalms/42.txt", include_str!("starter/bible/psalms/42.txt")),
    ("data/pages/bible/psalms/43.txt", include_str!("starter/bible/psalms/43.txt")),
    ("data/pages/bible/psalms/44.txt", include_str!("starter/bible/psalms/44.txt")),
    ("data/pages/bible/psalms/45.txt", include_str!("starter/bible/psalms/45.txt")),
    ("data/pages/bible/psalms/46.txt", include_str!("starter/bible/psalms/46.txt")),
    ("data/pages/bible/psalms/47.txt", include_str!("starter/bible/psalms/47.txt")),
    ("data/pages/bible/psalms/48.txt", include_str!("starter/bible/psalms/48.txt")),
    ("data/pages/bible/psalms/49.txt", include_str!("starter/bible/psalms/49.txt")),
    ("data/pages/bible/psalms/50.txt", include_str!("starter/bible/psalms/50.txt")),
    ("data/pages/bible/psalms/51.txt", include_str!("starter/bible/psalms/51.txt")),
    ("data/pages/bible/psalms/52.txt", include_str!("starter/bible/psalms/52.txt")),
    ("data/pages/bible/psalms/53.txt", include_str!("starter/bible/psalms/53.txt")),
    ("data/pages/bible/psalms/54.txt", include_str!("starter/bible/psalms/54.txt")),
    ("data/pages/bible/psalms/55.txt", include_str!("starter/bible/psalms/55.txt")),
    ("data/pages/bible/psalms/56.txt", include_str!("starter/bible/psalms/56.txt")),
    ("data/pages/bible/psalms/57.txt", include_str!("starter/bible/psalms/57.txt")),
    ("data/pages/bible/psalms/58.txt", include_str!("starter/bible/psalms/58.txt")),
    ("data/pages/bible/psalms/59.txt", include_str!("starter/bible/psalms/59.txt")),
    ("data/pages/bible/psalms/60.txt", include_str!("starter/bible/psalms/60.txt")),
    ("data/pages/bible/psalms/61.txt", include_str!("starter/bible/psalms/61.txt")),
    ("data/pages/bible/psalms/62.txt", include_str!("starter/bible/psalms/62.txt")),
    ("data/pages/bible/psalms/63.txt", include_str!("starter/bible/psalms/63.txt")),
    ("data/pages/bible/psalms/64.txt", include_str!("starter/bible/psalms/64.txt")),
    ("data/pages/bible/psalms/65.txt", include_str!("starter/bible/psalms/65.txt")),
    ("data/pages/bible/psalms/66.txt", include_str!("starter/bible/psalms/66.txt")),
    ("data/pages/bible/psalms/67.txt", include_str!("starter/bible/psalms/67.txt")),
    ("data/pages/bible/psalms/68.txt", include_str!("starter/bible/psalms/68.txt")),
    ("data/pages/bible/psalms/69.txt", include_str!("starter/bible/psalms/69.txt")),
    ("data/pages/bible/psalms/70.txt", include_str!("starter/bible/psalms/70.txt")),
    ("data/pages/bible/psalms/71.txt", include_str!("starter/bible/psalms/71.txt")),
    ("data/pages/bible/psalms/72.txt", include_str!("starter/bible/psalms/72.txt")),
    ("data/pages/bible/psalms/73.txt", include_str!("starter/bible/psalms/73.txt")),
    ("data/pages/bible/psalms/74.txt", include_str!("starter/bible/psalms/74.txt")),
    ("data/pages/bible/psalms/75.txt", include_str!("starter/bible/psalms/75.txt")),
    ("data/pages/bible/psalms/76.txt", include_str!("starter/bible/psalms/76.txt")),
    ("data/pages/bible/psalms/77.txt", include_str!("starter/bible/psalms/77.txt")),
    ("data/pages/bible/psalms/78.txt", include_str!("starter/bible/psalms/78.txt")),
    ("data/pages/bible/psalms/79.txt", include_str!("starter/bible/psalms/79.txt")),
    ("data/pages/bible/psalms/80.txt", include_str!("starter/bible/psalms/80.txt")),
    ("data/pages/bible/psalms/81.txt", include_str!("starter/bible/psalms/81.txt")),
    ("data/pages/bible/psalms/82.txt", include_str!("starter/bible/psalms/82.txt")),
    ("data/pages/bible/psalms/83.txt", include_str!("starter/bible/psalms/83.txt")),
    ("data/pages/bible/psalms/84.txt", include_str!("starter/bible/psalms/84.txt")),
    ("data/pages/bible/psalms/85.txt", include_str!("starter/bible/psalms/85.txt")),
    ("data/pages/bible/psalms/86.txt", include_str!("starter/bible/psalms/86.txt")),
    ("data/pages/bible/psalms/87.txt", include_str!("starter/bible/psalms/87.txt")),
    ("data/pages/bible/psalms/88.txt", include_str!("starter/bible/psalms/88.txt")),
    ("data/pages/bible/psalms/89.txt", include_str!("starter/bible/psalms/89.txt")),
    ("data/pages/bible/psalms/90.txt", include_str!("starter/bible/psalms/90.txt")),
    ("data/pages/bible/psalms/91.txt", include_str!("starter/bible/psalms/91.txt")),
    ("data/pages/bible/psalms/92.txt", include_str!("starter/bible/psalms/92.txt")),
    ("data/pages/bible/psalms/93.txt", include_str!("starter/bible/psalms/93.txt")),
    ("data/pages/bible/psalms/94.txt", include_str!("starter/bible/psalms/94.txt")),
    ("data/pages/bible/psalms/95.txt", include_str!("starter/bible/psalms/95.txt")),
    ("data/pages/bible/psalms/96.txt", include_str!("starter/bible/psalms/96.txt")),
    ("data/pages/bible/psalms/97.txt", include_str!("starter/bible/psalms/97.txt")),
    ("data/pages/bible/psalms/98.txt", include_str!("starter/bible/psalms/98.txt")),
    ("data/pages/bible/psalms/99.txt", include_str!("starter/bible/psalms/99.txt")),
    ("data/pages/bible/psalms/100.txt", include_str!("starter/bible/psalms/100.txt")),
    ("data/pages/bible/psalms/101.txt", include_str!("starter/bible/psalms/101.txt")),
    ("data/pages/bible/psalms/102.txt", include_str!("starter/bible/psalms/102.txt")),
    ("data/pages/bible/psalms/103.txt", include_str!("starter/bible/psalms/103.txt")),
    ("data/pages/bible/psalms/104.txt", include_str!("starter/bible/psalms/104.txt")),
    ("data/pages/bible/psalms/105.txt", include_str!("starter/bible/psalms/105.txt")),
    ("data/pages/bible/psalms/106.txt", include_str!("starter/bible/psalms/106.txt")),
    ("data/pages/bible/psalms/107.txt", include_str!("starter/bible/psalms/107.txt")),
    ("data/pages/bible/psalms/108.txt", include_str!("starter/bible/psalms/108.txt")),
    ("data/pages/bible/psalms/109.txt", include_str!("starter/bible/psalms/109.txt")),
    ("data/pages/bible/psalms/110.txt", include_str!("starter/bible/psalms/110.txt")),
    ("data/pages/bible/psalms/111.txt", include_str!("starter/bible/psalms/111.txt")),
    ("data/pages/bible/psalms/112.txt", include_str!("starter/bible/psalms/112.txt")),
    ("data/pages/bible/psalms/113.txt", include_str!("starter/bible/psalms/113.txt")),
    ("data/pages/bible/psalms/114.txt", include_str!("starter/bible/psalms/114.txt")),
    ("data/pages/bible/psalms/115.txt", include_str!("starter/bible/psalms/115.txt")),
    ("data/pages/bible/psalms/116.txt", include_str!("starter/bible/psalms/116.txt")),
    ("data/pages/bible/psalms/117.txt", include_str!("starter/bible/psalms/117.txt")),
    ("data/pages/bible/psalms/118.txt", include_str!("starter/bible/psalms/118.txt")),
    ("data/pages/bible/psalms/119.txt", include_str!("starter/bible/psalms/119.txt")),
    ("data/pages/bible/psalms/120.txt", include_str!("starter/bible/psalms/120.txt")),
    ("data/pages/bible/psalms/121.txt", include_str!("starter/bible/psalms/121.txt")),
    ("data/pages/bible/psalms/122.txt", include_str!("starter/bible/psalms/122.txt")),
    ("data/pages/bible/psalms/123.txt", include_str!("starter/bible/psalms/123.txt")),
    ("data/pages/bible/psalms/124.txt", include_str!("starter/bible/psalms/124.txt")),
    ("data/pages/bible/psalms/125.txt", include_str!("starter/bible/psalms/125.txt")),
    ("data/pages/bible/psalms/126.txt", include_str!("starter/bible/psalms/126.txt")),
    ("data/pages/bible/psalms/127.txt", include_str!("starter/bible/psalms/127.txt")),
    ("data/pages/bible/psalms/128.txt", include_str!("starter/bible/psalms/128.txt")),
    ("data/pages/bible/psalms/129.txt", include_str!("starter/bible/psalms/129.txt")),
    ("data/pages/bible/psalms/130.txt", include_str!("starter/bible/psalms/130.txt")),
    ("data/pages/bible/psalms/131.txt", include_str!("starter/bible/psalms/131.txt")),
    ("data/pages/bible/psalms/132.txt", include_str!("starter/bible/psalms/132.txt")),
    ("data/pages/bible/psalms/133.txt", include_str!("starter/bible/psalms/133.txt")),
    ("data/pages/bible/psalms/134.txt", include_str!("starter/bible/psalms/134.txt")),
    ("data/pages/bible/psalms/135.txt", include_str!("starter/bible/psalms/135.txt")),
    ("data/pages/bible/psalms/136.txt", include_str!("starter/bible/psalms/136.txt")),
    ("data/pages/bible/psalms/137.txt", include_str!("starter/bible/psalms/137.txt")),
    ("data/pages/bible/psalms/138.txt", include_str!("starter/bible/psalms/138.txt")),
    ("data/pages/bible/psalms/139.txt", include_str!("starter/bible/psalms/139.txt")),
    ("data/pages/bible/psalms/140.txt", include_str!("starter/bible/psalms/140.txt")),
    ("data/pages/bible/psalms/141.txt", include_str!("starter/bible/psalms/141.txt")),
    ("data/pages/bible/psalms/142.txt", include_str!("starter/bible/psalms/142.txt")),
    ("data/pages/bible/psalms/143.txt", include_str!("starter/bible/psalms/143.txt")),
    ("data/pages/bible/psalms/144.txt", include_str!("starter/bible/psalms/144.txt")),
    ("data/pages/bible/psalms/145.txt", include_str!("starter/bible/psalms/145.txt")),
    ("data/pages/bible/psalms/146.txt", include_str!("starter/bible/psalms/146.txt")),
    ("data/pages/bible/psalms/147.txt", include_str!("starter/bible/psalms/147.txt")),
    ("data/pages/bible/psalms/148.txt", include_str!("starter/bible/psalms/148.txt")),
    ("data/pages/bible/psalms/149.txt", include_str!("starter/bible/psalms/149.txt")),
    ("data/pages/bible/psalms/150.txt", include_str!("starter/bible/psalms/150.txt")),
    ("data/pages/bible/psalms/start.txt", include_str!("starter/bible/psalms/start.txt")),
    ("data/pages/bible/proverbs/1.txt", include_str!("starter/bible/proverbs/1.txt")),
    ("data/pages/bible/proverbs/2.txt", include_str!("starter/bible/proverbs/2.txt")),
    ("data/pages/bible/proverbs/3.txt", include_str!("starter/bible/proverbs/3.txt")),
    ("data/pages/bible/proverbs/4.txt", include_str!("starter/bible/proverbs/4.txt")),
    ("data/pages/bible/proverbs/5.txt", include_str!("starter/bible/proverbs/5.txt")),
    ("data/pages/bible/proverbs/6.txt", include_str!("starter/bible/proverbs/6.txt")),
    ("data/pages/bible/proverbs/7.txt", include_str!("starter/bible/proverbs/7.txt")),
    ("data/pages/bible/proverbs/8.txt", include_str!("starter/bible/proverbs/8.txt")),
    ("data/pages/bible/proverbs/9.txt", include_str!("starter/bible/proverbs/9.txt")),
    ("data/pages/bible/proverbs/10.txt", include_str!("starter/bible/proverbs/10.txt")),
    ("data/pages/bible/proverbs/11.txt", include_str!("starter/bible/proverbs/11.txt")),
    ("data/pages/bible/proverbs/12.txt", include_str!("starter/bible/proverbs/12.txt")),
    ("data/pages/bible/proverbs/13.txt", include_str!("starter/bible/proverbs/13.txt")),
    ("data/pages/bible/proverbs/14.txt", include_str!("starter/bible/proverbs/14.txt")),
    ("data/pages/bible/proverbs/15.txt", include_str!("starter/bible/proverbs/15.txt")),
    ("data/pages/bible/proverbs/16.txt", include_str!("starter/bible/proverbs/16.txt")),
    ("data/pages/bible/proverbs/17.txt", include_str!("starter/bible/proverbs/17.txt")),
    ("data/pages/bible/proverbs/18.txt", include_str!("starter/bible/proverbs/18.txt")),
    ("data/pages/bible/proverbs/19.txt", include_str!("starter/bible/proverbs/19.txt")),
    ("data/pages/bible/proverbs/20.txt", include_str!("starter/bible/proverbs/20.txt")),
    ("data/pages/bible/proverbs/21.txt", include_str!("starter/bible/proverbs/21.txt")),
    ("data/pages/bible/proverbs/22.txt", include_str!("starter/bible/proverbs/22.txt")),
    ("data/pages/bible/proverbs/23.txt", include_str!("starter/bible/proverbs/23.txt")),
    ("data/pages/bible/proverbs/24.txt", include_str!("starter/bible/proverbs/24.txt")),
    ("data/pages/bible/proverbs/25.txt", include_str!("starter/bible/proverbs/25.txt")),
    ("data/pages/bible/proverbs/26.txt", include_str!("starter/bible/proverbs/26.txt")),
    ("data/pages/bible/proverbs/27.txt", include_str!("starter/bible/proverbs/27.txt")),
    ("data/pages/bible/proverbs/28.txt", include_str!("starter/bible/proverbs/28.txt")),
    ("data/pages/bible/proverbs/29.txt", include_str!("starter/bible/proverbs/29.txt")),
    ("data/pages/bible/proverbs/30.txt", include_str!("starter/bible/proverbs/30.txt")),
    ("data/pages/bible/proverbs/31.txt", include_str!("starter/bible/proverbs/31.txt")),
    ("data/pages/bible/proverbs/start.txt", include_str!("starter/bible/proverbs/start.txt")),
    ("data/pages/bible/ecclesiastes/1.txt", include_str!("starter/bible/ecclesiastes/1.txt")),
    ("data/pages/bible/ecclesiastes/2.txt", include_str!("starter/bible/ecclesiastes/2.txt")),
    ("data/pages/bible/ecclesiastes/3.txt", include_str!("starter/bible/ecclesiastes/3.txt")),
    ("data/pages/bible/ecclesiastes/4.txt", include_str!("starter/bible/ecclesiastes/4.txt")),
    ("data/pages/bible/ecclesiastes/5.txt", include_str!("starter/bible/ecclesiastes/5.txt")),
    ("data/pages/bible/ecclesiastes/6.txt", include_str!("starter/bible/ecclesiastes/6.txt")),
    ("data/pages/bible/ecclesiastes/7.txt", include_str!("starter/bible/ecclesiastes/7.txt")),
    ("data/pages/bible/ecclesiastes/8.txt", include_str!("starter/bible/ecclesiastes/8.txt")),
    ("data/pages/bible/ecclesiastes/9.txt", include_str!("starter/bible/ecclesiastes/9.txt")),
    ("data/pages/bible/ecclesiastes/10.txt", include_str!("starter/bible/ecclesiastes/10.txt")),
    ("data/pages/bible/ecclesiastes/11.txt", include_str!("starter/bible/ecclesiastes/11.txt")),
    ("data/pages/bible/ecclesiastes/12.txt", include_str!("starter/bible/ecclesiastes/12.txt")),
    ("data/pages/bible/ecclesiastes/start.txt", include_str!("starter/bible/ecclesiastes/start.txt")),
    ("data/pages/bible/song_of_solomon/1.txt", include_str!("starter/bible/song_of_solomon/1.txt")),
    ("data/pages/bible/song_of_solomon/2.txt", include_str!("starter/bible/song_of_solomon/2.txt")),
    ("data/pages/bible/song_of_solomon/3.txt", include_str!("starter/bible/song_of_solomon/3.txt")),
    ("data/pages/bible/song_of_solomon/4.txt", include_str!("starter/bible/song_of_solomon/4.txt")),
    ("data/pages/bible/song_of_solomon/5.txt", include_str!("starter/bible/song_of_solomon/5.txt")),
    ("data/pages/bible/song_of_solomon/6.txt", include_str!("starter/bible/song_of_solomon/6.txt")),
    ("data/pages/bible/song_of_solomon/7.txt", include_str!("starter/bible/song_of_solomon/7.txt")),
    ("data/pages/bible/song_of_solomon/8.txt", include_str!("starter/bible/song_of_solomon/8.txt")),
    ("data/pages/bible/song_of_solomon/start.txt", include_str!("starter/bible/song_of_solomon/start.txt")),
    ("data/pages/bible/isaiah/1.txt", include_str!("starter/bible/isaiah/1.txt")),
    ("data/pages/bible/isaiah/2.txt", include_str!("starter/bible/isaiah/2.txt")),
    ("data/pages/bible/isaiah/3.txt", include_str!("starter/bible/isaiah/3.txt")),
    ("data/pages/bible/isaiah/4.txt", include_str!("starter/bible/isaiah/4.txt")),
    ("data/pages/bible/isaiah/5.txt", include_str!("starter/bible/isaiah/5.txt")),
    ("data/pages/bible/isaiah/6.txt", include_str!("starter/bible/isaiah/6.txt")),
    ("data/pages/bible/isaiah/7.txt", include_str!("starter/bible/isaiah/7.txt")),
    ("data/pages/bible/isaiah/8.txt", include_str!("starter/bible/isaiah/8.txt")),
    ("data/pages/bible/isaiah/9.txt", include_str!("starter/bible/isaiah/9.txt")),
    ("data/pages/bible/isaiah/10.txt", include_str!("starter/bible/isaiah/10.txt")),
    ("data/pages/bible/isaiah/11.txt", include_str!("starter/bible/isaiah/11.txt")),
    ("data/pages/bible/isaiah/12.txt", include_str!("starter/bible/isaiah/12.txt")),
    ("data/pages/bible/isaiah/13.txt", include_str!("starter/bible/isaiah/13.txt")),
    ("data/pages/bible/isaiah/14.txt", include_str!("starter/bible/isaiah/14.txt")),
    ("data/pages/bible/isaiah/15.txt", include_str!("starter/bible/isaiah/15.txt")),
    ("data/pages/bible/isaiah/16.txt", include_str!("starter/bible/isaiah/16.txt")),
    ("data/pages/bible/isaiah/17.txt", include_str!("starter/bible/isaiah/17.txt")),
    ("data/pages/bible/isaiah/18.txt", include_str!("starter/bible/isaiah/18.txt")),
    ("data/pages/bible/isaiah/19.txt", include_str!("starter/bible/isaiah/19.txt")),
    ("data/pages/bible/isaiah/20.txt", include_str!("starter/bible/isaiah/20.txt")),
    ("data/pages/bible/isaiah/21.txt", include_str!("starter/bible/isaiah/21.txt")),
    ("data/pages/bible/isaiah/22.txt", include_str!("starter/bible/isaiah/22.txt")),
    ("data/pages/bible/isaiah/23.txt", include_str!("starter/bible/isaiah/23.txt")),
    ("data/pages/bible/isaiah/24.txt", include_str!("starter/bible/isaiah/24.txt")),
    ("data/pages/bible/isaiah/25.txt", include_str!("starter/bible/isaiah/25.txt")),
    ("data/pages/bible/isaiah/26.txt", include_str!("starter/bible/isaiah/26.txt")),
    ("data/pages/bible/isaiah/27.txt", include_str!("starter/bible/isaiah/27.txt")),
    ("data/pages/bible/isaiah/28.txt", include_str!("starter/bible/isaiah/28.txt")),
    ("data/pages/bible/isaiah/29.txt", include_str!("starter/bible/isaiah/29.txt")),
    ("data/pages/bible/isaiah/30.txt", include_str!("starter/bible/isaiah/30.txt")),
    ("data/pages/bible/isaiah/31.txt", include_str!("starter/bible/isaiah/31.txt")),
    ("data/pages/bible/isaiah/32.txt", include_str!("starter/bible/isaiah/32.txt")),
    ("data/pages/bible/isaiah/33.txt", include_str!("starter/bible/isaiah/33.txt")),
    ("data/pages/bible/isaiah/34.txt", include_str!("starter/bible/isaiah/34.txt")),
    ("data/pages/bible/isaiah/35.txt", include_str!("starter/bible/isaiah/35.txt")),
    ("data/pages/bible/isaiah/36.txt", include_str!("starter/bible/isaiah/36.txt")),
    ("data/pages/bible/isaiah/37.txt", include_str!("starter/bible/isaiah/37.txt")),
    ("data/pages/bible/isaiah/38.txt", include_str!("starter/bible/isaiah/38.txt")),
    ("data/pages/bible/isaiah/39.txt", include_str!("starter/bible/isaiah/39.txt")),
    ("data/pages/bible/isaiah/40.txt", include_str!("starter/bible/isaiah/40.txt")),
    ("data/pages/bible/isaiah/41.txt", include_str!("starter/bible/isaiah/41.txt")),
    ("data/pages/bible/isaiah/42.txt", include_str!("starter/bible/isaiah/42.txt")),
    ("data/pages/bible/isaiah/43.txt", include_str!("starter/bible/isaiah/43.txt")),
    ("data/pages/bible/isaiah/44.txt", include_str!("starter/bible/isaiah/44.txt")),
    ("data/pages/bible/isaiah/45.txt", include_str!("starter/bible/isaiah/45.txt")),
    ("data/pages/bible/isaiah/46.txt", include_str!("starter/bible/isaiah/46.txt")),
    ("data/pages/bible/isaiah/47.txt", include_str!("starter/bible/isaiah/47.txt")),
    ("data/pages/bible/isaiah/48.txt", include_str!("starter/bible/isaiah/48.txt")),
    ("data/pages/bible/isaiah/49.txt", include_str!("starter/bible/isaiah/49.txt")),
    ("data/pages/bible/isaiah/50.txt", include_str!("starter/bible/isaiah/50.txt")),
    ("data/pages/bible/isaiah/51.txt", include_str!("starter/bible/isaiah/51.txt")),
    ("data/pages/bible/isaiah/52.txt", include_str!("starter/bible/isaiah/52.txt")),
    ("data/pages/bible/isaiah/53.txt", include_str!("starter/bible/isaiah/53.txt")),
    ("data/pages/bible/isaiah/54.txt", include_str!("starter/bible/isaiah/54.txt")),
    ("data/pages/bible/isaiah/55.txt", include_str!("starter/bible/isaiah/55.txt")),
    ("data/pages/bible/isaiah/56.txt", include_str!("starter/bible/isaiah/56.txt")),
    ("data/pages/bible/isaiah/57.txt", include_str!("starter/bible/isaiah/57.txt")),
    ("data/pages/bible/isaiah/58.txt", include_str!("starter/bible/isaiah/58.txt")),
    ("data/pages/bible/isaiah/59.txt", include_str!("starter/bible/isaiah/59.txt")),
    ("data/pages/bible/isaiah/60.txt", include_str!("starter/bible/isaiah/60.txt")),
    ("data/pages/bible/isaiah/61.txt", include_str!("starter/bible/isaiah/61.txt")),
    ("data/pages/bible/isaiah/62.txt", include_str!("starter/bible/isaiah/62.txt")),
    ("data/pages/bible/isaiah/63.txt", include_str!("starter/bible/isaiah/63.txt")),
    ("data/pages/bible/isaiah/64.txt", include_str!("starter/bible/isaiah/64.txt")),
    ("data/pages/bible/isaiah/65.txt", include_str!("starter/bible/isaiah/65.txt")),
    ("data/pages/bible/isaiah/66.txt", include_str!("starter/bible/isaiah/66.txt")),
    ("data/pages/bible/isaiah/start.txt", include_str!("starter/bible/isaiah/start.txt")),
    ("data/pages/bible/jeremiah/1.txt", include_str!("starter/bible/jeremiah/1.txt")),
    ("data/pages/bible/jeremiah/2.txt", include_str!("starter/bible/jeremiah/2.txt")),
    ("data/pages/bible/jeremiah/3.txt", include_str!("starter/bible/jeremiah/3.txt")),
    ("data/pages/bible/jeremiah/4.txt", include_str!("starter/bible/jeremiah/4.txt")),
    ("data/pages/bible/jeremiah/5.txt", include_str!("starter/bible/jeremiah/5.txt")),
    ("data/pages/bible/jeremiah/6.txt", include_str!("starter/bible/jeremiah/6.txt")),
    ("data/pages/bible/jeremiah/7.txt", include_str!("starter/bible/jeremiah/7.txt")),
    ("data/pages/bible/jeremiah/8.txt", include_str!("starter/bible/jeremiah/8.txt")),
    ("data/pages/bible/jeremiah/9.txt", include_str!("starter/bible/jeremiah/9.txt")),
    ("data/pages/bible/jeremiah/10.txt", include_str!("starter/bible/jeremiah/10.txt")),
    ("data/pages/bible/jeremiah/11.txt", include_str!("starter/bible/jeremiah/11.txt")),
    ("data/pages/bible/jeremiah/12.txt", include_str!("starter/bible/jeremiah/12.txt")),
    ("data/pages/bible/jeremiah/13.txt", include_str!("starter/bible/jeremiah/13.txt")),
    ("data/pages/bible/jeremiah/14.txt", include_str!("starter/bible/jeremiah/14.txt")),
    ("data/pages/bible/jeremiah/15.txt", include_str!("starter/bible/jeremiah/15.txt")),
    ("data/pages/bible/jeremiah/16.txt", include_str!("starter/bible/jeremiah/16.txt")),
    ("data/pages/bible/jeremiah/17.txt", include_str!("starter/bible/jeremiah/17.txt")),
    ("data/pages/bible/jeremiah/18.txt", include_str!("starter/bible/jeremiah/18.txt")),
    ("data/pages/bible/jeremiah/19.txt", include_str!("starter/bible/jeremiah/19.txt")),
    ("data/pages/bible/jeremiah/20.txt", include_str!("starter/bible/jeremiah/20.txt")),
    ("data/pages/bible/jeremiah/21.txt", include_str!("starter/bible/jeremiah/21.txt")),
    ("data/pages/bible/jeremiah/22.txt", include_str!("starter/bible/jeremiah/22.txt")),
    ("data/pages/bible/jeremiah/23.txt", include_str!("starter/bible/jeremiah/23.txt")),
    ("data/pages/bible/jeremiah/24.txt", include_str!("starter/bible/jeremiah/24.txt")),
    ("data/pages/bible/jeremiah/25.txt", include_str!("starter/bible/jeremiah/25.txt")),
    ("data/pages/bible/jeremiah/26.txt", include_str!("starter/bible/jeremiah/26.txt")),
    ("data/pages/bible/jeremiah/27.txt", include_str!("starter/bible/jeremiah/27.txt")),
    ("data/pages/bible/jeremiah/28.txt", include_str!("starter/bible/jeremiah/28.txt")),
    ("data/pages/bible/jeremiah/29.txt", include_str!("starter/bible/jeremiah/29.txt")),
    ("data/pages/bible/jeremiah/30.txt", include_str!("starter/bible/jeremiah/30.txt")),
    ("data/pages/bible/jeremiah/31.txt", include_str!("starter/bible/jeremiah/31.txt")),
    ("data/pages/bible/jeremiah/32.txt", include_str!("starter/bible/jeremiah/32.txt")),
    ("data/pages/bible/jeremiah/33.txt", include_str!("starter/bible/jeremiah/33.txt")),
    ("data/pages/bible/jeremiah/34.txt", include_str!("starter/bible/jeremiah/34.txt")),
    ("data/pages/bible/jeremiah/35.txt", include_str!("starter/bible/jeremiah/35.txt")),
    ("data/pages/bible/jeremiah/36.txt", include_str!("starter/bible/jeremiah/36.txt")),
    ("data/pages/bible/jeremiah/37.txt", include_str!("starter/bible/jeremiah/37.txt")),
    ("data/pages/bible/jeremiah/38.txt", include_str!("starter/bible/jeremiah/38.txt")),
    ("data/pages/bible/jeremiah/39.txt", include_str!("starter/bible/jeremiah/39.txt")),
    ("data/pages/bible/jeremiah/40.txt", include_str!("starter/bible/jeremiah/40.txt")),
    ("data/pages/bible/jeremiah/41.txt", include_str!("starter/bible/jeremiah/41.txt")),
    ("data/pages/bible/jeremiah/42.txt", include_str!("starter/bible/jeremiah/42.txt")),
    ("data/pages/bible/jeremiah/43.txt", include_str!("starter/bible/jeremiah/43.txt")),
    ("data/pages/bible/jeremiah/44.txt", include_str!("starter/bible/jeremiah/44.txt")),
    ("data/pages/bible/jeremiah/45.txt", include_str!("starter/bible/jeremiah/45.txt")),
    ("data/pages/bible/jeremiah/46.txt", include_str!("starter/bible/jeremiah/46.txt")),
    ("data/pages/bible/jeremiah/47.txt", include_str!("starter/bible/jeremiah/47.txt")),
    ("data/pages/bible/jeremiah/48.txt", include_str!("starter/bible/jeremiah/48.txt")),
    ("data/pages/bible/jeremiah/49.txt", include_str!("starter/bible/jeremiah/49.txt")),
    ("data/pages/bible/jeremiah/50.txt", include_str!("starter/bible/jeremiah/50.txt")),
    ("data/pages/bible/jeremiah/51.txt", include_str!("starter/bible/jeremiah/51.txt")),
    ("data/pages/bible/jeremiah/52.txt", include_str!("starter/bible/jeremiah/52.txt")),
    ("data/pages/bible/jeremiah/start.txt", include_str!("starter/bible/jeremiah/start.txt")),
    ("data/pages/bible/lamentations/1.txt", include_str!("starter/bible/lamentations/1.txt")),
    ("data/pages/bible/lamentations/2.txt", include_str!("starter/bible/lamentations/2.txt")),
    ("data/pages/bible/lamentations/3.txt", include_str!("starter/bible/lamentations/3.txt")),
    ("data/pages/bible/lamentations/4.txt", include_str!("starter/bible/lamentations/4.txt")),
    ("data/pages/bible/lamentations/5.txt", include_str!("starter/bible/lamentations/5.txt")),
    ("data/pages/bible/lamentations/start.txt", include_str!("starter/bible/lamentations/start.txt")),
    ("data/pages/bible/ezekiel/1.txt", include_str!("starter/bible/ezekiel/1.txt")),
    ("data/pages/bible/ezekiel/2.txt", include_str!("starter/bible/ezekiel/2.txt")),
    ("data/pages/bible/ezekiel/3.txt", include_str!("starter/bible/ezekiel/3.txt")),
    ("data/pages/bible/ezekiel/4.txt", include_str!("starter/bible/ezekiel/4.txt")),
    ("data/pages/bible/ezekiel/5.txt", include_str!("starter/bible/ezekiel/5.txt")),
    ("data/pages/bible/ezekiel/6.txt", include_str!("starter/bible/ezekiel/6.txt")),
    ("data/pages/bible/ezekiel/7.txt", include_str!("starter/bible/ezekiel/7.txt")),
    ("data/pages/bible/ezekiel/8.txt", include_str!("starter/bible/ezekiel/8.txt")),
    ("data/pages/bible/ezekiel/9.txt", include_str!("starter/bible/ezekiel/9.txt")),
    ("data/pages/bible/ezekiel/10.txt", include_str!("starter/bible/ezekiel/10.txt")),
    ("data/pages/bible/ezekiel/11.txt", include_str!("starter/bible/ezekiel/11.txt")),
    ("data/pages/bible/ezekiel/12.txt", include_str!("starter/bible/ezekiel/12.txt")),
    ("data/pages/bible/ezekiel/13.txt", include_str!("starter/bible/ezekiel/13.txt")),
    ("data/pages/bible/ezekiel/14.txt", include_str!("starter/bible/ezekiel/14.txt")),
    ("data/pages/bible/ezekiel/15.txt", include_str!("starter/bible/ezekiel/15.txt")),
    ("data/pages/bible/ezekiel/16.txt", include_str!("starter/bible/ezekiel/16.txt")),
    ("data/pages/bible/ezekiel/17.txt", include_str!("starter/bible/ezekiel/17.txt")),
    ("data/pages/bible/ezekiel/18.txt", include_str!("starter/bible/ezekiel/18.txt")),
    ("data/pages/bible/ezekiel/19.txt", include_str!("starter/bible/ezekiel/19.txt")),
    ("data/pages/bible/ezekiel/20.txt", include_str!("starter/bible/ezekiel/20.txt")),
    ("data/pages/bible/ezekiel/21.txt", include_str!("starter/bible/ezekiel/21.txt")),
    ("data/pages/bible/ezekiel/22.txt", include_str!("starter/bible/ezekiel/22.txt")),
    ("data/pages/bible/ezekiel/23.txt", include_str!("starter/bible/ezekiel/23.txt")),
    ("data/pages/bible/ezekiel/24.txt", include_str!("starter/bible/ezekiel/24.txt")),
    ("data/pages/bible/ezekiel/25.txt", include_str!("starter/bible/ezekiel/25.txt")),
    ("data/pages/bible/ezekiel/26.txt", include_str!("starter/bible/ezekiel/26.txt")),
    ("data/pages/bible/ezekiel/27.txt", include_str!("starter/bible/ezekiel/27.txt")),
    ("data/pages/bible/ezekiel/28.txt", include_str!("starter/bible/ezekiel/28.txt")),
    ("data/pages/bible/ezekiel/29.txt", include_str!("starter/bible/ezekiel/29.txt")),
    ("data/pages/bible/ezekiel/30.txt", include_str!("starter/bible/ezekiel/30.txt")),
    ("data/pages/bible/ezekiel/31.txt", include_str!("starter/bible/ezekiel/31.txt")),
    ("data/pages/bible/ezekiel/32.txt", include_str!("starter/bible/ezekiel/32.txt")),
    ("data/pages/bible/ezekiel/33.txt", include_str!("starter/bible/ezekiel/33.txt")),
    ("data/pages/bible/ezekiel/34.txt", include_str!("starter/bible/ezekiel/34.txt")),
    ("data/pages/bible/ezekiel/35.txt", include_str!("starter/bible/ezekiel/35.txt")),
    ("data/pages/bible/ezekiel/36.txt", include_str!("starter/bible/ezekiel/36.txt")),
    ("data/pages/bible/ezekiel/37.txt", include_str!("starter/bible/ezekiel/37.txt")),
    ("data/pages/bible/ezekiel/38.txt", include_str!("starter/bible/ezekiel/38.txt")),
    ("data/pages/bible/ezekiel/39.txt", include_str!("starter/bible/ezekiel/39.txt")),
    ("data/pages/bible/ezekiel/40.txt", include_str!("starter/bible/ezekiel/40.txt")),
    ("data/pages/bible/ezekiel/41.txt", include_str!("starter/bible/ezekiel/41.txt")),
    ("data/pages/bible/ezekiel/42.txt", include_str!("starter/bible/ezekiel/42.txt")),
    ("data/pages/bible/ezekiel/43.txt", include_str!("starter/bible/ezekiel/43.txt")),
    ("data/pages/bible/ezekiel/44.txt", include_str!("starter/bible/ezekiel/44.txt")),
    ("data/pages/bible/ezekiel/45.txt", include_str!("starter/bible/ezekiel/45.txt")),
    ("data/pages/bible/ezekiel/46.txt", include_str!("starter/bible/ezekiel/46.txt")),
    ("data/pages/bible/ezekiel/47.txt", include_str!("starter/bible/ezekiel/47.txt")),
    ("data/pages/bible/ezekiel/48.txt", include_str!("starter/bible/ezekiel/48.txt")),
    ("data/pages/bible/ezekiel/start.txt", include_str!("starter/bible/ezekiel/start.txt")),
    ("data/pages/bible/daniel/1.txt", include_str!("starter/bible/daniel/1.txt")),
    ("data/pages/bible/daniel/2.txt", include_str!("starter/bible/daniel/2.txt")),
    ("data/pages/bible/daniel/3.txt", include_str!("starter/bible/daniel/3.txt")),
    ("data/pages/bible/daniel/4.txt", include_str!("starter/bible/daniel/4.txt")),
    ("data/pages/bible/daniel/5.txt", include_str!("starter/bible/daniel/5.txt")),
    ("data/pages/bible/daniel/6.txt", include_str!("starter/bible/daniel/6.txt")),
    ("data/pages/bible/daniel/7.txt", include_str!("starter/bible/daniel/7.txt")),
    ("data/pages/bible/daniel/8.txt", include_str!("starter/bible/daniel/8.txt")),
    ("data/pages/bible/daniel/9.txt", include_str!("starter/bible/daniel/9.txt")),
    ("data/pages/bible/daniel/10.txt", include_str!("starter/bible/daniel/10.txt")),
    ("data/pages/bible/daniel/11.txt", include_str!("starter/bible/daniel/11.txt")),
    ("data/pages/bible/daniel/12.txt", include_str!("starter/bible/daniel/12.txt")),
    ("data/pages/bible/daniel/start.txt", include_str!("starter/bible/daniel/start.txt")),
    ("data/pages/bible/hosea/1.txt", include_str!("starter/bible/hosea/1.txt")),
    ("data/pages/bible/hosea/2.txt", include_str!("starter/bible/hosea/2.txt")),
    ("data/pages/bible/hosea/3.txt", include_str!("starter/bible/hosea/3.txt")),
    ("data/pages/bible/hosea/4.txt", include_str!("starter/bible/hosea/4.txt")),
    ("data/pages/bible/hosea/5.txt", include_str!("starter/bible/hosea/5.txt")),
    ("data/pages/bible/hosea/6.txt", include_str!("starter/bible/hosea/6.txt")),
    ("data/pages/bible/hosea/7.txt", include_str!("starter/bible/hosea/7.txt")),
    ("data/pages/bible/hosea/8.txt", include_str!("starter/bible/hosea/8.txt")),
    ("data/pages/bible/hosea/9.txt", include_str!("starter/bible/hosea/9.txt")),
    ("data/pages/bible/hosea/10.txt", include_str!("starter/bible/hosea/10.txt")),
    ("data/pages/bible/hosea/11.txt", include_str!("starter/bible/hosea/11.txt")),
    ("data/pages/bible/hosea/12.txt", include_str!("starter/bible/hosea/12.txt")),
    ("data/pages/bible/hosea/13.txt", include_str!("starter/bible/hosea/13.txt")),
    ("data/pages/bible/hosea/14.txt", include_str!("starter/bible/hosea/14.txt")),
    ("data/pages/bible/hosea/start.txt", include_str!("starter/bible/hosea/start.txt")),
    ("data/pages/bible/joel/1.txt", include_str!("starter/bible/joel/1.txt")),
    ("data/pages/bible/joel/2.txt", include_str!("starter/bible/joel/2.txt")),
    ("data/pages/bible/joel/3.txt", include_str!("starter/bible/joel/3.txt")),
    ("data/pages/bible/joel/start.txt", include_str!("starter/bible/joel/start.txt")),
    ("data/pages/bible/amos/1.txt", include_str!("starter/bible/amos/1.txt")),
    ("data/pages/bible/amos/2.txt", include_str!("starter/bible/amos/2.txt")),
    ("data/pages/bible/amos/3.txt", include_str!("starter/bible/amos/3.txt")),
    ("data/pages/bible/amos/4.txt", include_str!("starter/bible/amos/4.txt")),
    ("data/pages/bible/amos/5.txt", include_str!("starter/bible/amos/5.txt")),
    ("data/pages/bible/amos/6.txt", include_str!("starter/bible/amos/6.txt")),
    ("data/pages/bible/amos/7.txt", include_str!("starter/bible/amos/7.txt")),
    ("data/pages/bible/amos/8.txt", include_str!("starter/bible/amos/8.txt")),
    ("data/pages/bible/amos/9.txt", include_str!("starter/bible/amos/9.txt")),
    ("data/pages/bible/amos/start.txt", include_str!("starter/bible/amos/start.txt")),
    ("data/pages/bible/obadiah/1.txt", include_str!("starter/bible/obadiah/1.txt")),
    ("data/pages/bible/obadiah/start.txt", include_str!("starter/bible/obadiah/start.txt")),
    ("data/pages/bible/jonah/1.txt", include_str!("starter/bible/jonah/1.txt")),
    ("data/pages/bible/jonah/2.txt", include_str!("starter/bible/jonah/2.txt")),
    ("data/pages/bible/jonah/3.txt", include_str!("starter/bible/jonah/3.txt")),
    ("data/pages/bible/jonah/4.txt", include_str!("starter/bible/jonah/4.txt")),
    ("data/pages/bible/jonah/start.txt", include_str!("starter/bible/jonah/start.txt")),
    ("data/pages/bible/micah/1.txt", include_str!("starter/bible/micah/1.txt")),
    ("data/pages/bible/micah/2.txt", include_str!("starter/bible/micah/2.txt")),
    ("data/pages/bible/micah/3.txt", include_str!("starter/bible/micah/3.txt")),
    ("data/pages/bible/micah/4.txt", include_str!("starter/bible/micah/4.txt")),
    ("data/pages/bible/micah/5.txt", include_str!("starter/bible/micah/5.txt")),
    ("data/pages/bible/micah/6.txt", include_str!("starter/bible/micah/6.txt")),
    ("data/pages/bible/micah/7.txt", include_str!("starter/bible/micah/7.txt")),
    ("data/pages/bible/micah/start.txt", include_str!("starter/bible/micah/start.txt")),
    ("data/pages/bible/nahum/1.txt", include_str!("starter/bible/nahum/1.txt")),
    ("data/pages/bible/nahum/2.txt", include_str!("starter/bible/nahum/2.txt")),
    ("data/pages/bible/nahum/3.txt", include_str!("starter/bible/nahum/3.txt")),
    ("data/pages/bible/nahum/start.txt", include_str!("starter/bible/nahum/start.txt")),
    ("data/pages/bible/habakkuk/1.txt", include_str!("starter/bible/habakkuk/1.txt")),
    ("data/pages/bible/habakkuk/2.txt", include_str!("starter/bible/habakkuk/2.txt")),
    ("data/pages/bible/habakkuk/3.txt", include_str!("starter/bible/habakkuk/3.txt")),
    ("data/pages/bible/habakkuk/start.txt", include_str!("starter/bible/habakkuk/start.txt")),
    ("data/pages/bible/zephaniah/1.txt", include_str!("starter/bible/zephaniah/1.txt")),
    ("data/pages/bible/zephaniah/2.txt", include_str!("starter/bible/zephaniah/2.txt")),
    ("data/pages/bible/zephaniah/3.txt", include_str!("starter/bible/zephaniah/3.txt")),
    ("data/pages/bible/zephaniah/start.txt", include_str!("starter/bible/zephaniah/start.txt")),
    ("data/pages/bible/haggai/1.txt", include_str!("starter/bible/haggai/1.txt")),
    ("data/pages/bible/haggai/2.txt", include_str!("starter/bible/haggai/2.txt")),
    ("data/pages/bible/haggai/start.txt", include_str!("starter/bible/haggai/start.txt")),
    ("data/pages/bible/zechariah/1.txt", include_str!("starter/bible/zechariah/1.txt")),
    ("data/pages/bible/zechariah/2.txt", include_str!("starter/bible/zechariah/2.txt")),
    ("data/pages/bible/zechariah/3.txt", include_str!("starter/bible/zechariah/3.txt")),
    ("data/pages/bible/zechariah/4.txt", include_str!("starter/bible/zechariah/4.txt")),
    ("data/pages/bible/zechariah/5.txt", include_str!("starter/bible/zechariah/5.txt")),
    ("data/pages/bible/zechariah/6.txt", include_str!("starter/bible/zechariah/6.txt")),
    ("data/pages/bible/zechariah/7.txt", include_str!("starter/bible/zechariah/7.txt")),
    ("data/pages/bible/zechariah/8.txt", include_str!("starter/bible/zechariah/8.txt")),
    ("data/pages/bible/zechariah/9.txt", include_str!("starter/bible/zechariah/9.txt")),
    ("data/pages/bible/zechariah/10.txt", include_str!("starter/bible/zechariah/10.txt")),
    ("data/pages/bible/zechariah/11.txt", include_str!("starter/bible/zechariah/11.txt")),
    ("data/pages/bible/zechariah/12.txt", include_str!("starter/bible/zechariah/12.txt")),
    ("data/pages/bible/zechariah/13.txt", include_str!("starter/bible/zechariah/13.txt")),
    ("data/pages/bible/zechariah/14.txt", include_str!("starter/bible/zechariah/14.txt")),
    ("data/pages/bible/zechariah/start.txt", include_str!("starter/bible/zechariah/start.txt")),
    ("data/pages/bible/malachi/1.txt", include_str!("starter/bible/malachi/1.txt")),
    ("data/pages/bible/malachi/2.txt", include_str!("starter/bible/malachi/2.txt")),
    ("data/pages/bible/malachi/3.txt", include_str!("starter/bible/malachi/3.txt")),
    ("data/pages/bible/malachi/4.txt", include_str!("starter/bible/malachi/4.txt")),
    ("data/pages/bible/malachi/start.txt", include_str!("starter/bible/malachi/start.txt")),
    ("data/pages/bible/matthew/1.txt", include_str!("starter/bible/matthew/1.txt")),
    ("data/pages/bible/matthew/2.txt", include_str!("starter/bible/matthew/2.txt")),
    ("data/pages/bible/matthew/3.txt", include_str!("starter/bible/matthew/3.txt")),
    ("data/pages/bible/matthew/4.txt", include_str!("starter/bible/matthew/4.txt")),
    ("data/pages/bible/matthew/5.txt", include_str!("starter/bible/matthew/5.txt")),
    ("data/pages/bible/matthew/6.txt", include_str!("starter/bible/matthew/6.txt")),
    ("data/pages/bible/matthew/7.txt", include_str!("starter/bible/matthew/7.txt")),
    ("data/pages/bible/matthew/8.txt", include_str!("starter/bible/matthew/8.txt")),
    ("data/pages/bible/matthew/9.txt", include_str!("starter/bible/matthew/9.txt")),
    ("data/pages/bible/matthew/10.txt", include_str!("starter/bible/matthew/10.txt")),
    ("data/pages/bible/matthew/11.txt", include_str!("starter/bible/matthew/11.txt")),
    ("data/pages/bible/matthew/12.txt", include_str!("starter/bible/matthew/12.txt")),
    ("data/pages/bible/matthew/13.txt", include_str!("starter/bible/matthew/13.txt")),
    ("data/pages/bible/matthew/14.txt", include_str!("starter/bible/matthew/14.txt")),
    ("data/pages/bible/matthew/15.txt", include_str!("starter/bible/matthew/15.txt")),
    ("data/pages/bible/matthew/16.txt", include_str!("starter/bible/matthew/16.txt")),
    ("data/pages/bible/matthew/17.txt", include_str!("starter/bible/matthew/17.txt")),
    ("data/pages/bible/matthew/18.txt", include_str!("starter/bible/matthew/18.txt")),
    ("data/pages/bible/matthew/19.txt", include_str!("starter/bible/matthew/19.txt")),
    ("data/pages/bible/matthew/20.txt", include_str!("starter/bible/matthew/20.txt")),
    ("data/pages/bible/matthew/21.txt", include_str!("starter/bible/matthew/21.txt")),
    ("data/pages/bible/matthew/22.txt", include_str!("starter/bible/matthew/22.txt")),
    ("data/pages/bible/matthew/23.txt", include_str!("starter/bible/matthew/23.txt")),
    ("data/pages/bible/matthew/24.txt", include_str!("starter/bible/matthew/24.txt")),
    ("data/pages/bible/matthew/25.txt", include_str!("starter/bible/matthew/25.txt")),
    ("data/pages/bible/matthew/26.txt", include_str!("starter/bible/matthew/26.txt")),
    ("data/pages/bible/matthew/27.txt", include_str!("starter/bible/matthew/27.txt")),
    ("data/pages/bible/matthew/28.txt", include_str!("starter/bible/matthew/28.txt")),
    ("data/pages/bible/matthew/start.txt", include_str!("starter/bible/matthew/start.txt")),
    ("data/pages/bible/mark/1.txt", include_str!("starter/bible/mark/1.txt")),
    ("data/pages/bible/mark/2.txt", include_str!("starter/bible/mark/2.txt")),
    ("data/pages/bible/mark/3.txt", include_str!("starter/bible/mark/3.txt")),
    ("data/pages/bible/mark/4.txt", include_str!("starter/bible/mark/4.txt")),
    ("data/pages/bible/mark/5.txt", include_str!("starter/bible/mark/5.txt")),
    ("data/pages/bible/mark/6.txt", include_str!("starter/bible/mark/6.txt")),
    ("data/pages/bible/mark/7.txt", include_str!("starter/bible/mark/7.txt")),
    ("data/pages/bible/mark/8.txt", include_str!("starter/bible/mark/8.txt")),
    ("data/pages/bible/mark/9.txt", include_str!("starter/bible/mark/9.txt")),
    ("data/pages/bible/mark/10.txt", include_str!("starter/bible/mark/10.txt")),
    ("data/pages/bible/mark/11.txt", include_str!("starter/bible/mark/11.txt")),
    ("data/pages/bible/mark/12.txt", include_str!("starter/bible/mark/12.txt")),
    ("data/pages/bible/mark/13.txt", include_str!("starter/bible/mark/13.txt")),
    ("data/pages/bible/mark/14.txt", include_str!("starter/bible/mark/14.txt")),
    ("data/pages/bible/mark/15.txt", include_str!("starter/bible/mark/15.txt")),
    ("data/pages/bible/mark/16.txt", include_str!("starter/bible/mark/16.txt")),
    ("data/pages/bible/mark/start.txt", include_str!("starter/bible/mark/start.txt")),
    ("data/pages/bible/luke/1.txt", include_str!("starter/bible/luke/1.txt")),
    ("data/pages/bible/luke/2.txt", include_str!("starter/bible/luke/2.txt")),
    ("data/pages/bible/luke/3.txt", include_str!("starter/bible/luke/3.txt")),
    ("data/pages/bible/luke/4.txt", include_str!("starter/bible/luke/4.txt")),
    ("data/pages/bible/luke/5.txt", include_str!("starter/bible/luke/5.txt")),
    ("data/pages/bible/luke/6.txt", include_str!("starter/bible/luke/6.txt")),
    ("data/pages/bible/luke/7.txt", include_str!("starter/bible/luke/7.txt")),
    ("data/pages/bible/luke/8.txt", include_str!("starter/bible/luke/8.txt")),
    ("data/pages/bible/luke/9.txt", include_str!("starter/bible/luke/9.txt")),
    ("data/pages/bible/luke/10.txt", include_str!("starter/bible/luke/10.txt")),
    ("data/pages/bible/luke/11.txt", include_str!("starter/bible/luke/11.txt")),
    ("data/pages/bible/luke/12.txt", include_str!("starter/bible/luke/12.txt")),
    ("data/pages/bible/luke/13.txt", include_str!("starter/bible/luke/13.txt")),
    ("data/pages/bible/luke/14.txt", include_str!("starter/bible/luke/14.txt")),
    ("data/pages/bible/luke/15.txt", include_str!("starter/bible/luke/15.txt")),
    ("data/pages/bible/luke/16.txt", include_str!("starter/bible/luke/16.txt")),
    ("data/pages/bible/luke/17.txt", include_str!("starter/bible/luke/17.txt")),
    ("data/pages/bible/luke/18.txt", include_str!("starter/bible/luke/18.txt")),
    ("data/pages/bible/luke/19.txt", include_str!("starter/bible/luke/19.txt")),
    ("data/pages/bible/luke/20.txt", include_str!("starter/bible/luke/20.txt")),
    ("data/pages/bible/luke/21.txt", include_str!("starter/bible/luke/21.txt")),
    ("data/pages/bible/luke/22.txt", include_str!("starter/bible/luke/22.txt")),
    ("data/pages/bible/luke/23.txt", include_str!("starter/bible/luke/23.txt")),
    ("data/pages/bible/luke/24.txt", include_str!("starter/bible/luke/24.txt")),
    ("data/pages/bible/luke/start.txt", include_str!("starter/bible/luke/start.txt")),
    ("data/pages/bible/john/1.txt", include_str!("starter/bible/john/1.txt")),
    ("data/pages/bible/john/2.txt", include_str!("starter/bible/john/2.txt")),
    ("data/pages/bible/john/3.txt", include_str!("starter/bible/john/3.txt")),
    ("data/pages/bible/john/4.txt", include_str!("starter/bible/john/4.txt")),
    ("data/pages/bible/john/5.txt", include_str!("starter/bible/john/5.txt")),
    ("data/pages/bible/john/6.txt", include_str!("starter/bible/john/6.txt")),
    ("data/pages/bible/john/7.txt", include_str!("starter/bible/john/7.txt")),
    ("data/pages/bible/john/8.txt", include_str!("starter/bible/john/8.txt")),
    ("data/pages/bible/john/9.txt", include_str!("starter/bible/john/9.txt")),
    ("data/pages/bible/john/10.txt", include_str!("starter/bible/john/10.txt")),
    ("data/pages/bible/john/11.txt", include_str!("starter/bible/john/11.txt")),
    ("data/pages/bible/john/12.txt", include_str!("starter/bible/john/12.txt")),
    ("data/pages/bible/john/13.txt", include_str!("starter/bible/john/13.txt")),
    ("data/pages/bible/john/14.txt", include_str!("starter/bible/john/14.txt")),
    ("data/pages/bible/john/15.txt", include_str!("starter/bible/john/15.txt")),
    ("data/pages/bible/john/16.txt", include_str!("starter/bible/john/16.txt")),
    ("data/pages/bible/john/17.txt", include_str!("starter/bible/john/17.txt")),
    ("data/pages/bible/john/18.txt", include_str!("starter/bible/john/18.txt")),
    ("data/pages/bible/john/19.txt", include_str!("starter/bible/john/19.txt")),
    ("data/pages/bible/john/20.txt", include_str!("starter/bible/john/20.txt")),
    ("data/pages/bible/john/21.txt", include_str!("starter/bible/john/21.txt")),
    ("data/pages/bible/john/start.txt", include_str!("starter/bible/john/start.txt")),
    ("data/pages/bible/acts/1.txt", include_str!("starter/bible/acts/1.txt")),
    ("data/pages/bible/acts/2.txt", include_str!("starter/bible/acts/2.txt")),
    ("data/pages/bible/acts/3.txt", include_str!("starter/bible/acts/3.txt")),
    ("data/pages/bible/acts/4.txt", include_str!("starter/bible/acts/4.txt")),
    ("data/pages/bible/acts/5.txt", include_str!("starter/bible/acts/5.txt")),
    ("data/pages/bible/acts/6.txt", include_str!("starter/bible/acts/6.txt")),
    ("data/pages/bible/acts/7.txt", include_str!("starter/bible/acts/7.txt")),
    ("data/pages/bible/acts/8.txt", include_str!("starter/bible/acts/8.txt")),
    ("data/pages/bible/acts/9.txt", include_str!("starter/bible/acts/9.txt")),
    ("data/pages/bible/acts/10.txt", include_str!("starter/bible/acts/10.txt")),
    ("data/pages/bible/acts/11.txt", include_str!("starter/bible/acts/11.txt")),
    ("data/pages/bible/acts/12.txt", include_str!("starter/bible/acts/12.txt")),
    ("data/pages/bible/acts/13.txt", include_str!("starter/bible/acts/13.txt")),
    ("data/pages/bible/acts/14.txt", include_str!("starter/bible/acts/14.txt")),
    ("data/pages/bible/acts/15.txt", include_str!("starter/bible/acts/15.txt")),
    ("data/pages/bible/acts/16.txt", include_str!("starter/bible/acts/16.txt")),
    ("data/pages/bible/acts/17.txt", include_str!("starter/bible/acts/17.txt")),
    ("data/pages/bible/acts/18.txt", include_str!("starter/bible/acts/18.txt")),
    ("data/pages/bible/acts/19.txt", include_str!("starter/bible/acts/19.txt")),
    ("data/pages/bible/acts/20.txt", include_str!("starter/bible/acts/20.txt")),
    ("data/pages/bible/acts/21.txt", include_str!("starter/bible/acts/21.txt")),
    ("data/pages/bible/acts/22.txt", include_str!("starter/bible/acts/22.txt")),
    ("data/pages/bible/acts/23.txt", include_str!("starter/bible/acts/23.txt")),
    ("data/pages/bible/acts/24.txt", include_str!("starter/bible/acts/24.txt")),
    ("data/pages/bible/acts/25.txt", include_str!("starter/bible/acts/25.txt")),
    ("data/pages/bible/acts/26.txt", include_str!("starter/bible/acts/26.txt")),
    ("data/pages/bible/acts/27.txt", include_str!("starter/bible/acts/27.txt")),
    ("data/pages/bible/acts/28.txt", include_str!("starter/bible/acts/28.txt")),
    ("data/pages/bible/acts/start.txt", include_str!("starter/bible/acts/start.txt")),
    ("data/pages/bible/romans/1.txt", include_str!("starter/bible/romans/1.txt")),
    ("data/pages/bible/romans/2.txt", include_str!("starter/bible/romans/2.txt")),
    ("data/pages/bible/romans/3.txt", include_str!("starter/bible/romans/3.txt")),
    ("data/pages/bible/romans/4.txt", include_str!("starter/bible/romans/4.txt")),
    ("data/pages/bible/romans/5.txt", include_str!("starter/bible/romans/5.txt")),
    ("data/pages/bible/romans/6.txt", include_str!("starter/bible/romans/6.txt")),
    ("data/pages/bible/romans/7.txt", include_str!("starter/bible/romans/7.txt")),
    ("data/pages/bible/romans/8.txt", include_str!("starter/bible/romans/8.txt")),
    ("data/pages/bible/romans/9.txt", include_str!("starter/bible/romans/9.txt")),
    ("data/pages/bible/romans/10.txt", include_str!("starter/bible/romans/10.txt")),
    ("data/pages/bible/romans/11.txt", include_str!("starter/bible/romans/11.txt")),
    ("data/pages/bible/romans/12.txt", include_str!("starter/bible/romans/12.txt")),
    ("data/pages/bible/romans/13.txt", include_str!("starter/bible/romans/13.txt")),
    ("data/pages/bible/romans/14.txt", include_str!("starter/bible/romans/14.txt")),
    ("data/pages/bible/romans/15.txt", include_str!("starter/bible/romans/15.txt")),
    ("data/pages/bible/romans/16.txt", include_str!("starter/bible/romans/16.txt")),
    ("data/pages/bible/romans/start.txt", include_str!("starter/bible/romans/start.txt")),
    ("data/pages/bible/1_corinthians/1.txt", include_str!("starter/bible/1_corinthians/1.txt")),
    ("data/pages/bible/1_corinthians/2.txt", include_str!("starter/bible/1_corinthians/2.txt")),
    ("data/pages/bible/1_corinthians/3.txt", include_str!("starter/bible/1_corinthians/3.txt")),
    ("data/pages/bible/1_corinthians/4.txt", include_str!("starter/bible/1_corinthians/4.txt")),
    ("data/pages/bible/1_corinthians/5.txt", include_str!("starter/bible/1_corinthians/5.txt")),
    ("data/pages/bible/1_corinthians/6.txt", include_str!("starter/bible/1_corinthians/6.txt")),
    ("data/pages/bible/1_corinthians/7.txt", include_str!("starter/bible/1_corinthians/7.txt")),
    ("data/pages/bible/1_corinthians/8.txt", include_str!("starter/bible/1_corinthians/8.txt")),
    ("data/pages/bible/1_corinthians/9.txt", include_str!("starter/bible/1_corinthians/9.txt")),
    ("data/pages/bible/1_corinthians/10.txt", include_str!("starter/bible/1_corinthians/10.txt")),
    ("data/pages/bible/1_corinthians/11.txt", include_str!("starter/bible/1_corinthians/11.txt")),
    ("data/pages/bible/1_corinthians/12.txt", include_str!("starter/bible/1_corinthians/12.txt")),
    ("data/pages/bible/1_corinthians/13.txt", include_str!("starter/bible/1_corinthians/13.txt")),
    ("data/pages/bible/1_corinthians/14.txt", include_str!("starter/bible/1_corinthians/14.txt")),
    ("data/pages/bible/1_corinthians/15.txt", include_str!("starter/bible/1_corinthians/15.txt")),
    ("data/pages/bible/1_corinthians/16.txt", include_str!("starter/bible/1_corinthians/16.txt")),
    ("data/pages/bible/1_corinthians/start.txt", include_str!("starter/bible/1_corinthians/start.txt")),
    ("data/pages/bible/2_corinthians/1.txt", include_str!("starter/bible/2_corinthians/1.txt")),
    ("data/pages/bible/2_corinthians/2.txt", include_str!("starter/bible/2_corinthians/2.txt")),
    ("data/pages/bible/2_corinthians/3.txt", include_str!("starter/bible/2_corinthians/3.txt")),
    ("data/pages/bible/2_corinthians/4.txt", include_str!("starter/bible/2_corinthians/4.txt")),
    ("data/pages/bible/2_corinthians/5.txt", include_str!("starter/bible/2_corinthians/5.txt")),
    ("data/pages/bible/2_corinthians/6.txt", include_str!("starter/bible/2_corinthians/6.txt")),
    ("data/pages/bible/2_corinthians/7.txt", include_str!("starter/bible/2_corinthians/7.txt")),
    ("data/pages/bible/2_corinthians/8.txt", include_str!("starter/bible/2_corinthians/8.txt")),
    ("data/pages/bible/2_corinthians/9.txt", include_str!("starter/bible/2_corinthians/9.txt")),
    ("data/pages/bible/2_corinthians/10.txt", include_str!("starter/bible/2_corinthians/10.txt")),
    ("data/pages/bible/2_corinthians/11.txt", include_str!("starter/bible/2_corinthians/11.txt")),
    ("data/pages/bible/2_corinthians/12.txt", include_str!("starter/bible/2_corinthians/12.txt")),
    ("data/pages/bible/2_corinthians/13.txt", include_str!("starter/bible/2_corinthians/13.txt")),
    ("data/pages/bible/2_corinthians/start.txt", include_str!("starter/bible/2_corinthians/start.txt")),
    ("data/pages/bible/galatians/1.txt", include_str!("starter/bible/galatians/1.txt")),
    ("data/pages/bible/galatians/2.txt", include_str!("starter/bible/galatians/2.txt")),
    ("data/pages/bible/galatians/3.txt", include_str!("starter/bible/galatians/3.txt")),
    ("data/pages/bible/galatians/4.txt", include_str!("starter/bible/galatians/4.txt")),
    ("data/pages/bible/galatians/5.txt", include_str!("starter/bible/galatians/5.txt")),
    ("data/pages/bible/galatians/6.txt", include_str!("starter/bible/galatians/6.txt")),
    ("data/pages/bible/galatians/start.txt", include_str!("starter/bible/galatians/start.txt")),
    ("data/pages/bible/ephesians/1.txt", include_str!("starter/bible/ephesians/1.txt")),
    ("data/pages/bible/ephesians/2.txt", include_str!("starter/bible/ephesians/2.txt")),
    ("data/pages/bible/ephesians/3.txt", include_str!("starter/bible/ephesians/3.txt")),
    ("data/pages/bible/ephesians/4.txt", include_str!("starter/bible/ephesians/4.txt")),
    ("data/pages/bible/ephesians/5.txt", include_str!("starter/bible/ephesians/5.txt")),
    ("data/pages/bible/ephesians/6.txt", include_str!("starter/bible/ephesians/6.txt")),
    ("data/pages/bible/ephesians/start.txt", include_str!("starter/bible/ephesians/start.txt")),
    ("data/pages/bible/philippians/1.txt", include_str!("starter/bible/philippians/1.txt")),
    ("data/pages/bible/philippians/2.txt", include_str!("starter/bible/philippians/2.txt")),
    ("data/pages/bible/philippians/3.txt", include_str!("starter/bible/philippians/3.txt")),
    ("data/pages/bible/philippians/4.txt", include_str!("starter/bible/philippians/4.txt")),
    ("data/pages/bible/philippians/start.txt", include_str!("starter/bible/philippians/start.txt")),
    ("data/pages/bible/colossians/1.txt", include_str!("starter/bible/colossians/1.txt")),
    ("data/pages/bible/colossians/2.txt", include_str!("starter/bible/colossians/2.txt")),
    ("data/pages/bible/colossians/3.txt", include_str!("starter/bible/colossians/3.txt")),
    ("data/pages/bible/colossians/4.txt", include_str!("starter/bible/colossians/4.txt")),
    ("data/pages/bible/colossians/start.txt", include_str!("starter/bible/colossians/start.txt")),
    ("data/pages/bible/1_thessalonians/1.txt", include_str!("starter/bible/1_thessalonians/1.txt")),
    ("data/pages/bible/1_thessalonians/2.txt", include_str!("starter/bible/1_thessalonians/2.txt")),
    ("data/pages/bible/1_thessalonians/3.txt", include_str!("starter/bible/1_thessalonians/3.txt")),
    ("data/pages/bible/1_thessalonians/4.txt", include_str!("starter/bible/1_thessalonians/4.txt")),
    ("data/pages/bible/1_thessalonians/5.txt", include_str!("starter/bible/1_thessalonians/5.txt")),
    ("data/pages/bible/1_thessalonians/start.txt", include_str!("starter/bible/1_thessalonians/start.txt")),
    ("data/pages/bible/2_thessalonians/1.txt", include_str!("starter/bible/2_thessalonians/1.txt")),
    ("data/pages/bible/2_thessalonians/2.txt", include_str!("starter/bible/2_thessalonians/2.txt")),
    ("data/pages/bible/2_thessalonians/3.txt", include_str!("starter/bible/2_thessalonians/3.txt")),
    ("data/pages/bible/2_thessalonians/start.txt", include_str!("starter/bible/2_thessalonians/start.txt")),
    ("data/pages/bible/1_timothy/1.txt", include_str!("starter/bible/1_timothy/1.txt")),
    ("data/pages/bible/1_timothy/2.txt", include_str!("starter/bible/1_timothy/2.txt")),
    ("data/pages/bible/1_timothy/3.txt", include_str!("starter/bible/1_timothy/3.txt")),
    ("data/pages/bible/1_timothy/4.txt", include_str!("starter/bible/1_timothy/4.txt")),
    ("data/pages/bible/1_timothy/5.txt", include_str!("starter/bible/1_timothy/5.txt")),
    ("data/pages/bible/1_timothy/6.txt", include_str!("starter/bible/1_timothy/6.txt")),
    ("data/pages/bible/1_timothy/start.txt", include_str!("starter/bible/1_timothy/start.txt")),
    ("data/pages/bible/2_timothy/1.txt", include_str!("starter/bible/2_timothy/1.txt")),
    ("data/pages/bible/2_timothy/2.txt", include_str!("starter/bible/2_timothy/2.txt")),
    ("data/pages/bible/2_timothy/3.txt", include_str!("starter/bible/2_timothy/3.txt")),
    ("data/pages/bible/2_timothy/4.txt", include_str!("starter/bible/2_timothy/4.txt")),
    ("data/pages/bible/2_timothy/start.txt", include_str!("starter/bible/2_timothy/start.txt")),
    ("data/pages/bible/titus/1.txt", include_str!("starter/bible/titus/1.txt")),
    ("data/pages/bible/titus/2.txt", include_str!("starter/bible/titus/2.txt")),
    ("data/pages/bible/titus/3.txt", include_str!("starter/bible/titus/3.txt")),
    ("data/pages/bible/titus/start.txt", include_str!("starter/bible/titus/start.txt")),
    ("data/pages/bible/philemon/1.txt", include_str!("starter/bible/philemon/1.txt")),
    ("data/pages/bible/philemon/start.txt", include_str!("starter/bible/philemon/start.txt")),
    ("data/pages/bible/hebrews/1.txt", include_str!("starter/bible/hebrews/1.txt")),
    ("data/pages/bible/hebrews/2.txt", include_str!("starter/bible/hebrews/2.txt")),
    ("data/pages/bible/hebrews/3.txt", include_str!("starter/bible/hebrews/3.txt")),
    ("data/pages/bible/hebrews/4.txt", include_str!("starter/bible/hebrews/4.txt")),
    ("data/pages/bible/hebrews/5.txt", include_str!("starter/bible/hebrews/5.txt")),
    ("data/pages/bible/hebrews/6.txt", include_str!("starter/bible/hebrews/6.txt")),
    ("data/pages/bible/hebrews/7.txt", include_str!("starter/bible/hebrews/7.txt")),
    ("data/pages/bible/hebrews/8.txt", include_str!("starter/bible/hebrews/8.txt")),
    ("data/pages/bible/hebrews/9.txt", include_str!("starter/bible/hebrews/9.txt")),
    ("data/pages/bible/hebrews/10.txt", include_str!("starter/bible/hebrews/10.txt")),
    ("data/pages/bible/hebrews/11.txt", include_str!("starter/bible/hebrews/11.txt")),
    ("data/pages/bible/hebrews/12.txt", include_str!("starter/bible/hebrews/12.txt")),
    ("data/pages/bible/hebrews/13.txt", include_str!("starter/bible/hebrews/13.txt")),
    ("data/pages/bible/hebrews/start.txt", include_str!("starter/bible/hebrews/start.txt")),
    ("data/pages/bible/james/1.txt", include_str!("starter/bible/james/1.txt")),
    ("data/pages/bible/james/2.txt", include_str!("starter/bible/james/2.txt")),
    ("data/pages/bible/james/3.txt", include_str!("starter/bible/james/3.txt")),
    ("data/pages/bible/james/4.txt", include_str!("starter/bible/james/4.txt")),
    ("data/pages/bible/james/5.txt", include_str!("starter/bible/james/5.txt")),
    ("data/pages/bible/james/start.txt", include_str!("starter/bible/james/start.txt")),
    ("data/pages/bible/1_peter/1.txt", include_str!("starter/bible/1_peter/1.txt")),
    ("data/pages/bible/1_peter/2.txt", include_str!("starter/bible/1_peter/2.txt")),
    ("data/pages/bible/1_peter/3.txt", include_str!("starter/bible/1_peter/3.txt")),
    ("data/pages/bible/1_peter/4.txt", include_str!("starter/bible/1_peter/4.txt")),
    ("data/pages/bible/1_peter/5.txt", include_str!("starter/bible/1_peter/5.txt")),
    ("data/pages/bible/1_peter/start.txt", include_str!("starter/bible/1_peter/start.txt")),
    ("data/pages/bible/2_peter/1.txt", include_str!("starter/bible/2_peter/1.txt")),
    ("data/pages/bible/2_peter/2.txt", include_str!("starter/bible/2_peter/2.txt")),
    ("data/pages/bible/2_peter/3.txt", include_str!("starter/bible/2_peter/3.txt")),
    ("data/pages/bible/2_peter/start.txt", include_str!("starter/bible/2_peter/start.txt")),
    ("data/pages/bible/1_john/1.txt", include_str!("starter/bible/1_john/1.txt")),
    ("data/pages/bible/1_john/2.txt", include_str!("starter/bible/1_john/2.txt")),
    ("data/pages/bible/1_john/3.txt", include_str!("starter/bible/1_john/3.txt")),
    ("data/pages/bible/1_john/4.txt", include_str!("starter/bible/1_john/4.txt")),
    ("data/pages/bible/1_john/5.txt", include_str!("starter/bible/1_john/5.txt")),
    ("data/pages/bible/1_john/start.txt", include_str!("starter/bible/1_john/start.txt")),
    ("data/pages/bible/2_john/1.txt", include_str!("starter/bible/2_john/1.txt")),
    ("data/pages/bible/2_john/start.txt", include_str!("starter/bible/2_john/start.txt")),
    ("data/pages/bible/3_john/1.txt", include_str!("starter/bible/3_john/1.txt")),
    ("data/pages/bible/3_john/start.txt", include_str!("starter/bible/3_john/start.txt")),
    ("data/pages/bible/jude/1.txt", include_str!("starter/bible/jude/1.txt")),
    ("data/pages/bible/jude/start.txt", include_str!("starter/bible/jude/start.txt")),
    ("data/pages/bible/revelation/1.txt", include_str!("starter/bible/revelation/1.txt")),
    ("data/pages/bible/revelation/2.txt", include_str!("starter/bible/revelation/2.txt")),
    ("data/pages/bible/revelation/3.txt", include_str!("starter/bible/revelation/3.txt")),
    ("data/pages/bible/revelation/4.txt", include_str!("starter/bible/revelation/4.txt")),
    ("data/pages/bible/revelation/5.txt", include_str!("starter/bible/revelation/5.txt")),
    ("data/pages/bible/revelation/6.txt", include_str!("starter/bible/revelation/6.txt")),
    ("data/pages/bible/revelation/7.txt", include_str!("starter/bible/revelation/7.txt")),
    ("data/pages/bible/revelation/8.txt", include_str!("starter/bible/revelation/8.txt")),
    ("data/pages/bible/revelation/9.txt", include_str!("starter/bible/revelation/9.txt")),
    ("data/pages/bible/revelation/10.txt", include_str!("starter/bible/revelation/10.txt")),
    ("data/pages/bible/revelation/11.txt", include_str!("starter/bible/revelation/11.txt")),
    ("data/pages/bible/revelation/12.txt", include_str!("starter/bible/revelation/12.txt")),
    ("data/pages/bible/revelation/13.txt", include_str!("starter/bible/revelation/13.txt")),
    ("data/pages/bible/revelation/14.txt", include_str!("starter/bible/revelation/14.txt")),
    ("data/pages/bible/revelation/15.txt", include_str!("starter/bible/revelation/15.txt")),
    ("data/pages/bible/revelation/16.txt", include_str!("starter/bible/revelation/16.txt")),
    ("data/pages/bible/revelation/17.txt", include_str!("starter/bible/revelation/17.txt")),
    ("data/pages/bible/revelation/18.txt", include_str!("starter/bible/revelation/18.txt")),
    ("data/pages/bible/revelation/19.txt", include_str!("starter/bible/revelation/19.txt")),
    ("data/pages/bible/revelation/20.txt", include_str!("starter/bible/revelation/20.txt")),
    ("data/pages/bible/revelation/21.txt", include_str!("starter/bible/revelation/21.txt")),
    ("data/pages/bible/revelation/22.txt", include_str!("starter/bible/revelation/22.txt")),
    ("data/pages/bible/revelation/start.txt", include_str!("starter/bible/revelation/start.txt")),
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
    ("data/pages/genealogy/start.txt", include_str!("starter/legacy/genealogy_start_v5.txt")),
    ("data/pages/genealogy/chart.txt", include_str!("starter/legacy/genealogy_chart_v5.txt")),
];

// Automatic backups: at most one per day, the newest N are kept.
const AUTO_BACKUP_INTERVAL_SECS: u64 = 24 * 60 * 60;
const AUTO_BACKUP_KEEP: usize = 10;

// Update source. VERSION on the stable branch is tiny and reliable; the tarball is the official release build.
const VERSION_URL: &str = "https://raw.githubusercontent.com/dokuwiki/dokuwiki/stable/VERSION";
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

fn release_asset_sha256(repository: &str, tag: &str, asset_name: &str) -> Result<String> {
    let url = format!("https://api.github.com/repos/{repository}/releases/tags/{tag}");
    let body = curl_text(&url).context("could not retrieve the release's integrity metadata")?;
    release_asset_sha256_from_json(&body, asset_name)
}

fn release_asset_sha256_from_json(body: &str, asset_name: &str) -> Result<String> {
    let release: serde_json::Value = serde_json::from_str(body)
        .context("GitHub returned invalid release metadata")?;
    let assets = release["assets"]
        .as_array()
        .context("GitHub release metadata has no asset list")?;
    let asset = assets
        .iter()
        .find(|asset| asset["name"].as_str() == Some(asset_name))
        .with_context(|| format!("GitHub release is missing {asset_name}"))?;
    let digest = asset["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .with_context(|| format!("GitHub has no SHA-256 digest for {asset_name}"))?;
    if digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()) {
        bail!("GitHub returned an invalid SHA-256 digest for {asset_name}");
    }
    Ok(digest.to_ascii_lowercase())
}

#[derive(Debug)]
enum UserEvent {
    Menu(MenuEvent),
    StartupAction(String),
    StartupProfileSelected(Result<Option<WikiProfile>, String>),
    StartupWikiCreated(Result<WikiProfile, String>),
    /// The user chose a backup zip to restore (already validated and confirmed).
    RestoreBackup { profile_id: String, zip: PathBuf },
    /// A new or imported wiki has been prepared and is ready to open.
    WikiCreated(Result<WikiProfile, String>),
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
    ApplyUpdate { profile_id: String, staging: PathBuf, version: String },
}

#[derive(Clone, Debug)]
struct WikiProfile {
    id: String,
    name: String,
    root: PathBuf,
    wiki: PathBuf,
}

const DEFAULT_WIKI_ID: &str = "default";
const DEFAULT_WIKI_NAME: &str = "Default Wiki";

#[derive(Clone)]
struct Layout {
    php: PathBuf,        // installed copy (Application Support/runtime/php) — what actually runs
    bundle_php: PathBuf, // pristine copy inside the .app
    runtime: PathBuf,
    bundle_wiki: PathBuf,
    wiki: PathBuf,
    session_preload: Option<PathBuf>,
    router: PathBuf,
    log: PathBuf,
    pidfile: PathBuf,
    bundle_auth: PathBuf, // pristine auth helper inside the .app
    bundle_plugins: PathBuf, // curated DokuWiki plugins shipped inside the .app
    auth_helper: PathBuf, // locally prepared copy that actually runs
    updates: PathBuf,
    php_updates: PathBuf,
    rollback: PathBuf,
    support: PathBuf,
    profile_root: PathBuf,
    profile_id: String,
    profile_name: String,
    token: String, // per-launch secret: only the app window can talk to the local wiki server
}

impl Layout {
    fn discover() -> Result<Self> {
        let res = resource_dir()?;
        let support = dirs::data_dir()
            .ok_or_else(|| anyhow!("Cannot locate ~/Library/Application Support"))?
            .join(APP_NAME);
        fs::create_dir_all(&support)?;
        let profile = active_wiki_profile(&support)?;
        let profile_root = profile.root.clone();
        Ok(Self {
            php: support.join("runtime").join("php"),
            bundle_php: res.join("php"),
            runtime: support.join("runtime"),
            bundle_wiki: res.join("dokuwiki"),
            wiki: profile.wiki,
            session_preload: (profile.id != DEFAULT_WIKI_ID).then(|| profile_root.join("session.php")),
            router: support.join("router.php"),
            log: support.join("php-server.log"),
            pidfile: support.join("php.pid"),
            bundle_auth: res.join("auth-helper"),
            bundle_plugins: res.join("plugins"),
            auth_helper: support.join("runtime").join("auth-helper"),
            updates: profile_root.join("updates"),
            php_updates: support.join("php-updates"),
            rollback: profile_root.join("wiki-rollback"),
            support,
            profile_root,
            profile_id: profile.id,
            profile_name: profile.name,
            token: random_token(),
        })
    }

    fn for_profile(&self, profile: &WikiProfile) -> Self {
        let mut layout = self.clone();
        layout.wiki = profile.wiki.clone();
        layout.session_preload = (profile.id != DEFAULT_WIKI_ID).then(|| profile.root.join("session.php"));
        layout.updates = profile.root.join("updates");
        layout.rollback = profile.root.join("wiki-rollback");
        layout.profile_root = profile.root.clone();
        layout.profile_id = profile.id.clone();
        layout.profile_name = profile.name.clone();
        layout.token = random_token();
        layout
    }
}

fn install_profile_session(l: &Layout) -> Result<()> {
    let Some(path) = &l.session_preload else { return Ok(()) };
    let name = profile_session_name(&l.profile_id)?.ok_or_else(|| anyhow!("The default wiki should not need a session preload."))?;
    let script = format!("<?php\nif (!defined('DOKU_SESSION_NAME')) define('DOKU_SESSION_NAME', '{name}');\n");
    fs::write(path, script)?;
    Ok(())
}

fn profile_session_name(profile_id: &str) -> Result<Option<String>> {
    if profile_id == DEFAULT_WIKI_ID {
        return Ok(None);
    }
    let id = profile_id
        .strip_prefix("wiki-")
        .filter(|_| valid_wiki_profile_id(profile_id))
        .ok_or_else(|| anyhow!("The wiki profile has an invalid session identifier."))?;
    Ok(Some(format!("ArchiveWiki_{id}")))
}

fn validate_wiki_name(name: &str) -> Result<String> {
    let name = name.trim();
    let length = name.chars().count();
    if length == 0
        || length > 64
        || name.len() > 200
        || name == "."
        || name == ".."
        || name.chars().any(|c| c.is_control() || matches!(c, '/' | '\\' | ':'))
    {
        bail!("Choose a wiki name of 1–64 characters that fits in a folder name and contains no slashes or control characters.");
    }
    Ok(name.to_owned())
}

fn valid_wiki_profile_id(id: &str) -> bool {
    id.strip_prefix("wiki-")
        .map(|suffix| suffix.len() == 32 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
        .unwrap_or(false)
}

fn wiki_profiles(support: &Path) -> Result<Vec<WikiProfile>> {
    let profiles_root = support.join("wikis");
    fs::create_dir_all(&profiles_root)?;
    let default_root = profiles_root.join(DEFAULT_WIKI_ID);
    fs::create_dir_all(&default_root)?;
    let default_name_file = default_root.join("name");
    if !default_name_file.exists() {
        fs::write(&default_name_file, DEFAULT_WIKI_NAME)?;
    }
    for file in ["backup-auto-off", "last-auto-backup", "update-skipped"] {
        let legacy = support.join(file);
        let current = default_root.join(file);
        if !current.exists() && legacy.is_file() && fs::rename(&legacy, &current).is_err() {
            fs::copy(&legacy, &current)?;
            fs::remove_file(legacy)?;
        }
    }
    for directory in ["updates", "wiki-rollback", "restore-stage", "restore-previous", "restore-failed", "snapshot-staging", "snapshot-verify"] {
        let legacy = support.join(directory);
        let current = default_root.join(directory);
        if !current.exists() && legacy.is_dir() {
            fs::rename(legacy, current)?;
        }
    }
    let default_name = validate_wiki_name(&fs::read_to_string(default_name_file)?)?;
    let mut profiles = vec![WikiProfile {
        id: DEFAULT_WIKI_ID.to_owned(),
        name: default_name,
        root: default_root,
        wiki: support.join("wiki"),
    }];

    for entry in fs::read_dir(&profiles_root)? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let id = entry.file_name().to_string_lossy().into_owned();
        if !valid_wiki_profile_id(&id) {
            continue;
        }
        let root = entry.path();
        let wiki = root.join("wiki");
        if !wiki.join("doku.php").is_file() {
            continue;
        }
        let name_file = root.join("name");
        if !name_file.is_file() {
            continue;
        }
        profiles.push(WikiProfile {
            id,
            name: validate_wiki_name(&fs::read_to_string(name_file)?)?,
            root,
            wiki,
        });
    }
    profiles.sort_by_key(|profile| profile.name.to_lowercase());
    Ok(profiles)
}

fn active_wiki_profile(support: &Path) -> Result<WikiProfile> {
    let profiles = wiki_profiles(support)?;
    let active_file = support.join("active-wiki");
    let active_id = match fs::read_to_string(&active_file) {
        Ok(id) => id.trim().to_owned(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => DEFAULT_WIKI_ID.to_owned(),
        Err(e) => return Err(e.into()),
    };
    if let Some(profile) = profiles.iter().find(|profile| profile.id == active_id) {
        return Ok(profile.clone());
    }
    profiles
        .into_iter()
        .find(|profile| profile.id == DEFAULT_WIKI_ID)
        .ok_or_else(|| anyhow!("The default wiki profile is unavailable."))
}

fn write_profile_name(profile: &WikiProfile, name: &str) -> Result<()> {
    let temp = profile.root.join(format!(".name-{}", random_token()));
    let result = (|| -> Result<()> {
        fs::write(&temp, name)?;
        fs::rename(&temp, profile.root.join("name"))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

fn move_backup_directory(old: &Path, new: &Path) -> Result<()> {
    if !old.exists() || fs::canonicalize(old).ok() == fs::canonicalize(new).ok() {
        return Ok(());
    }
    if !new.exists() {
        fs::rename(old, new).context("could not update the wiki's backup folder name")?;
        return Ok(());
    }
    if !old.is_dir() || !new.is_dir() {
        bail!("A wiki backup folder exists at an unexpected path.");
    }

    let mut entries = fs::read_dir(old)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    if entries.iter().any(|entry| !entry.file_type().map(|kind| kind.is_file()).unwrap_or(false)) {
        bail!("The wiki backup folder contains an unexpected subfolder or file type and cannot be safely renamed.");
    }

    let mut moved = Vec::new();
    for entry in entries {
        let source = entry.path();
        let destination = unique_path(new.join(entry.file_name()));
        if let Err(error) = fs::rename(&source, &destination) {
            let rollback_errors = moved
                .iter()
                .filter_map(|(from, to): &(PathBuf, PathBuf)| fs::rename(to, from).err())
                .collect::<Vec<_>>();
            if rollback_errors.is_empty() {
                return Err(error).context("could not move the wiki's existing backups");
            }
            bail!("Could not move the wiki's existing backups ({error}); some files could not be moved back: {}", rollback_errors[0]);
        }
        moved.push((source, destination));
    }
    if let Err(error) = fs::remove_dir(old) {
        let rollback_errors = moved
            .iter()
            .filter_map(|(from, to): &(PathBuf, PathBuf)| fs::rename(to, from).err())
            .collect::<Vec<_>>();
        if rollback_errors.is_empty() {
            return Err(error).context("could not remove the old wiki backup folder");
        }
        bail!("Could not remove the old wiki backup folder ({error}); some backups could not be moved back: {}", rollback_errors[0]);
    }
    Ok(())
}

fn rename_wiki_profile(
    support: &Path,
    profile_id: &str,
    new_name: &str,
    backups_root: Option<&Path>,
) -> Result<WikiProfile> {
    let new_name = validate_wiki_name(new_name)?;
    let mut profiles = wiki_profiles(support)?;
    let index = profiles
        .iter()
        .position(|profile| profile.id == profile_id)
        .ok_or_else(|| anyhow!("The selected wiki could not be found."))?;
    let profile = profiles[index].clone();
    if profiles.iter().any(|other| other.id != profile.id && other.name.eq_ignore_ascii_case(&new_name)) {
        bail!("A wiki named \"{new_name}\" already exists. Choose a different name.");
    }
    if profile.name == new_name {
        return Ok(profile);
    }

    let _guard = WIKI_IO_LOCK.lock().map_err(|_| anyhow!("Wiki file operation lock is poisoned"))?;
    write_profile_name(&profile, &new_name)?;
    if profile.id != DEFAULT_WIKI_ID {
        let migration = (|| -> Result<()> {
            let root = backups_root.ok_or_else(|| anyhow!("Cannot locate the wiki backup folder."))?;
            let old = root.join(profile_folder_name_for(&profile.name, &profile.id));
            let new = root.join(profile_folder_name_for(&new_name, &profile.id));
            move_backup_directory(&old, &new)
        })();
        if let Err(error) = migration {
            let rollback = write_profile_name(&profile, &profile.name);
            return match rollback {
                Ok(()) => Err(error),
                Err(rollback_error) => bail!(
                    "The wiki was renamed, but its backup folder could not be updated ({error}) and the old name could not be restored ({rollback_error})."
                ),
            };
        }
    }
    profiles[index].name = new_name;
    Ok(profiles.remove(index))
}

fn delete_wiki_profile(support: &Path, profile_id: &str, active_profile_id: &str) -> Result<()> {
    if profile_id == DEFAULT_WIKI_ID {
        bail!("The built-in Default Wiki is protected and cannot be deleted.");
    }
    if profile_id == active_profile_id {
        bail!("Switch to another wiki before deleting this one.");
    }
    if !valid_wiki_profile_id(profile_id) {
        bail!("The selected wiki has an invalid profile identifier.");
    }
    let root = support.join("wikis").join(profile_id);
    let profiles = wiki_profiles(support)?;
    let profile = profiles
        .iter()
        .find(|profile| profile.id == profile_id)
        .ok_or_else(|| anyhow!("The selected wiki could not be found."))?;
    if profile.root != root || profile.wiki != root.join("wiki") {
        bail!("The selected wiki profile is not stored in its expected location.");
    }

    let _guard = WIKI_IO_LOCK.lock().map_err(|_| anyhow!("Wiki file operation lock is poisoned"))?;
    fs::remove_dir_all(root).context("could not delete the selected wiki")
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

fn create_wiki_profile(l: &Layout, name: &str, import: Option<&Path>) -> Result<WikiProfile> {
    let name = validate_wiki_name(name)?;
    let profiles = wiki_profiles(&l.support)?;
    if profiles.iter().any(|profile| profile.name.eq_ignore_ascii_case(&name)) {
        bail!("A wiki named \"{name}\" already exists. Choose a different name.");
    }
    if let Some(zip) = import {
        validate_backup_zip(zip).context("the selected file isn't a valid ArchiveWiki backup")?;
    }

    let id = format!("wiki-{}", random_token());
    let root = l.support.join("wikis").join(&id);
    fs::create_dir(&root)?;
    let profile = WikiProfile { id, name, wiki: root.join("wiki"), root };
    let result = (|| -> Result<()> {
        fs::write(profile.root.join("name"), &profile.name)?;
        let target = l.for_profile(&profile);
        install_profile_session(&target)?;
        sync_wiki(&target.bundle_wiki, &target.wiki)?;
        install_plugins(&target);
        if let Some(zip) = import {
            restore_backup(&target, zip, false).context("could not import the backup")?;
            sync_wiki(&target.bundle_wiki, &target.wiki)?;
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_dir_all(&profile.root);
        return Err(e);
    }
    Ok(profile)
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
    text = text.replace(BROKEN_JOURNAL_BUTTON_CARDS_CSS, "");
    if have < 2 {
        text.push_str(BRANDING_CSS);
    }
    if have < 3 {
        text.push_str(THEME_CSS);
    }
    if have < 4 {
        text.push_str(JOURNAL_BUTTONS_CSS);
    }
    if have < 7 {
        text.push_str(JOURNAL_BUTTON_CARDS_CSS);
    }
    if have < 8 {
        text.push_str(HOME_ACTION_CARDS_CSS);
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

/// Starter pages, namespace templates and the friendly "page not found" text. Pristine shipped pages can be refreshed;
/// customized files are preserved, and missing starter files are added when the starter version changes.
fn install_starter(wiki: &Path) {
    let marker = wiki.join(".archivewiki-starter");
    let have = fs::read_to_string(&marker).ok().and_then(|t| t.trim().parse::<u32>().ok()).unwrap_or(0);
    if have >= STARTER_VERSION {
        return;
    }
    starter::replace_if_unchanged(
        wiki,
        &[
            (
                "data/pages/start.txt",
                include_str!("starter/legacy/start_v26.txt"),
                include_str!("starter/start.txt"),
            ),
            (
                "data/pages/sidebar.txt",
                include_str!("starter/legacy/sidebar_v26.txt"),
                include_str!("starter/sidebar.txt"),
            ),
            (
                "data/pages/journal/start.txt",
                include_str!("starter/legacy/journal_start_v26.txt"),
                include_str!("starter/journal_start.txt"),
            ),
            (
                "data/pages/genealogy/start.txt",
                include_str!("starter/legacy/genealogy_start_v26.txt"),
                include_str!("starter/genealogy_start.txt"),
            ),
            (
                "data/pages/genealogy/families/start.txt",
                include_str!("starter/legacy/genealogy_families_start_v26.txt"),
                include_str!("starter/genealogy_families_start.txt"),
            ),
            (
                "data/pages/genealogy/families/start.txt",
                include_str!("starter/legacy/genealogy_families_start_v27.txt"),
                include_str!("starter/genealogy_families_start.txt"),
            ),
            (
                "data/pages/genealogy/chart.txt",
                include_str!("starter/legacy/genealogy_chart_v27.txt"),
                include_str!("starter/genealogy_chart.txt"),
            ),
            (
                "data/pages/portfolio/start.txt",
                include_str!("starter/legacy/portfolio_start_v26.txt"),
                include_str!("starter/portfolio_start.txt"),
            ),
            (
                "data/pages/reference/start.txt",
                include_str!("starter/legacy/reference_start_v26.txt"),
                include_str!("starter/reference_start.txt"),
            ),
            (
                "data/pages/start.txt",
                include_str!("starter/legacy/start_v25.txt"),
                include_str!("starter/start.txt"),
            ),
            (
                "data/pages/start.txt",
                include_str!("starter/legacy/start_v24.txt"),
                include_str!("starter/start.txt"),
            ),
            (
                "data/pages/start.txt",
                include_str!("starter/legacy/start_v23.txt"),
                include_str!("starter/start.txt"),
            ),
            (
                "data/pages/start.txt",
                include_str!("starter/legacy/start_v22.txt"),
                include_str!("starter/start.txt"),
            ),
            (
                "data/pages/start.txt",
                include_str!("starter/legacy/start_v6.txt"),
                include_str!("starter/start.txt"),
            ),
            (
                "data/pages/start.txt",
                include_str!("starter/legacy/start_v7.txt"),
                include_str!("starter/start.txt"),
            ),
            (
                "data/pages/sidebar.txt",
                include_str!("starter/legacy/sidebar_v17.txt"),
                include_str!("starter/sidebar.txt"),
            ),
            (
                "data/pages/bible/start.txt",
                include_str!("starter/legacy/bible_start_v18.txt"),
                include_str!("starter/bible/start.txt"),
            ),
            (
                "data/pages/bible/start.txt",
                include_str!("starter/legacy/bible_start_v19.txt"),
                include_str!("starter/bible/start.txt"),
            ),
            (
                "data/pages/genealogy/start.txt",
                include_str!("starter/legacy/genealogy_start_v20.txt"),
                include_str!("starter/genealogy_start.txt"),
            ),
            (
                "data/pages/genealogy/_template.txt",
                include_str!("starter/legacy/genealogy_template_v20.txt"),
                include_str!("starter/genealogy_template.txt"),
            ),
            (
                "data/pages/genealogy/chart.txt",
                include_str!("starter/legacy/genealogy_chart_v20.txt"),
                include_str!("starter/genealogy_chart.txt"),
            ),
            (
                "data/pages/genealogy/start.txt",
                include_str!("starter/legacy/genealogy_start_v21.txt"),
                include_str!("starter/genealogy_start.txt"),
            ),
            (
                "data/pages/genealogy/_template.txt",
                include_str!("starter/legacy/genealogy_template_v21.txt"),
                include_str!("starter/genealogy_template.txt"),
            ),
            (
                "data/pages/genealogy/chart.txt",
                include_str!("starter/legacy/genealogy_chart_v21.txt"),
                include_str!("starter/genealogy_chart.txt"),
            ),
            (
                "data/pages/bible/notes/start.txt",
                include_str!("starter/legacy/bible_notes_start_v18.txt"),
                include_str!("starter/bible/notes/start.txt"),
            ),
            (
                "data/pages/bible/notes/_template.txt",
                include_str!("starter/legacy/bible_notes_template_v18.txt"),
                include_str!("starter/bible/notes/_template.txt"),
            ),
            (
                "data/pages/journal/start.txt",
                include_str!("starter/legacy/journal_start_v7.txt"),
                include_str!("starter/journal_start.txt"),
            ),
            (
                "data/pages/journal/start.txt",
                include_str!("starter/legacy/journal_start_v8.txt"),
                include_str!("starter/journal_start.txt"),
            ),
            (
                "data/pages/journal/start.txt",
                include_str!("starter/legacy/journal_start_v9.txt"),
                include_str!("starter/journal_start.txt"),
            ),
            (
                "data/pages/journal/start.txt",
                include_str!("starter/legacy/journal_start_v10.txt"),
                include_str!("starter/journal_start.txt"),
            ),
            (
                "data/pages/portfolio/start.txt",
                include_str!("starter/legacy/portfolio_start_v1.txt"),
                include_str!("starter/portfolio_start.txt"),
            ),
            (
                "data/pages/portfolio/start.txt",
                include_str!("starter/legacy/portfolio_start_v2.txt"),
                include_str!("starter/portfolio_start.txt"),
            ),
            (
                "data/pages/portfolio/start.txt",
                include_str!("starter/legacy/portfolio_start_v3.txt"),
                include_str!("starter/portfolio_start.txt"),
            ),
            (
                "data/pages/portfolio/residential.txt",
                include_str!("starter/legacy/portfolio_residential_v1.txt"),
                include_str!("starter/portfolio_residential.txt"),
            ),
            (
                "data/pages/portfolio/residential.txt",
                include_str!("starter/legacy/portfolio_residential_v2.txt"),
                include_str!("starter/portfolio_residential.txt"),
            ),
            (
                "data/pages/portfolio/employment.txt",
                include_str!("starter/legacy/portfolio_employment_v1.txt"),
                include_str!("starter/portfolio_employment.txt"),
            ),
            (
                "data/pages/portfolio/employment.txt",
                include_str!("starter/legacy/portfolio_employment_v2.txt"),
                include_str!("starter/portfolio_employment.txt"),
            ),
            (
                "data/pages/portfolio/employment.txt",
                include_str!("starter/legacy/portfolio_employment_v3.txt"),
                include_str!("starter/portfolio_employment.txt"),
            ),
            (
                "data/pages/portfolio/credentials.txt",
                include_str!("starter/legacy/portfolio_credentials_v1.txt"),
                include_str!("starter/portfolio_credentials.txt"),
            ),
            (
                "data/pages/portfolio/credentials.txt",
                include_str!("starter/legacy/portfolio_credentials_v2.txt"),
                include_str!("starter/portfolio_credentials.txt"),
            ),
            (
                "data/pages/portfolio/credentials.txt",
                include_str!("starter/legacy/portfolio_credentials_v3.txt"),
                include_str!("starter/portfolio_credentials.txt"),
            ),
            (
                "data/pages/journal/_template.txt",
                include_str!("starter/legacy/journal_template_v7.txt"),
                include_str!("starter/journal_template.txt"),
            ),
            (
                "data/pages/journal/_template.txt",
                include_str!("starter/legacy/journal_template_v8.txt"),
                include_str!("starter/journal_template.txt"),
            ),
        ],
    );
    starter::retire_legacy(wiki, LEGACY);
    starter::write_missing(wiki, STARTER);
    starter::write_missing(wiki, deuterocanon::PAGES);
    let _ = fs::write(&marker, STARTER_VERSION.to_string());
}

/// Curated plugins shipped with the app. Existing plugins are not overwritten, except the prior ArchiveWiki
/// Genealogy build, which is upgraded only after an exact match against its shipped files.
const AWGENEALOGY_V1_FILES: &[(&str, &str)] = &[
    ("action.php", include_str!("starter/legacy/awgenealogy_v1/action.php")),
    ("helper.php", include_str!("starter/legacy/awgenealogy_v1/helper.php")),
    ("plugin.info.txt", include_str!("starter/legacy/awgenealogy_v1/plugin.info.txt")),
    ("print.css", include_str!("starter/legacy/awgenealogy_v1/print.css")),
    ("script.js", include_str!("starter/legacy/awgenealogy_v1/script.js")),
    ("style.css", include_str!("starter/legacy/awgenealogy_v1/style.css")),
    ("syntax/pedigree.php", include_str!("starter/legacy/awgenealogy_v1/syntax/pedigree.php")),
    ("syntax/person.php", include_str!("starter/legacy/awgenealogy_v1/syntax/person.php")),
];
const AWGENEALOGY_V2_HASHES: &[(&str, &str)] = &[
    ("action.php", "7e7e5068db229baa78c04357c5c725f112726fc909303a1fc2d7bc9a8f809e7f"),
    ("helper.php", "0e54b8835fdb672877a8a8f3c2cf3fe811623439373ac06fb20efd6bbe36ed07"),
    ("plugin.info.txt", "0dd36ea60f1bf661b02a3eae079c49f21513d79afa4b7c8314165190e7cd1bc2"),
    ("print.css", "5a93c3b14d911850c3e299b241310f644e7684dab1cbfa60f6af8b239013a471"),
    ("script.js", "b9e548ff885eb081945b3b0f2fd75572bae7d9fa27576ded6f412f77983d753a"),
    ("style.css", "88a16bed4222fce19888adc1eaef9250a5323b92f626aa4c4e0f164f2c22f779"),
    ("syntax/directory.php", "2a4d51bbbc257f9fd590408ffb266a62e80c6a41cdfabcee9df6f8126f630ed9"),
    ("syntax/familygroup.php", "2b3f3c283030574f90461df4bef0eb0206721b9dfa0545d3b827a23c165793c6"),
    ("syntax/newperson.php", "993b11a382f96057887e3803ea6bba03395b6cc17cb8599342ef343b4e93d442"),
    ("syntax/pedigree.php", "3bc5624907770ac3b080655c9d2fcf79d2d05bfad85e645ed96ead9b99da0705"),
    ("syntax/person.php", "f3d1dc8286a2c65d71ef1e252c36ff277960446216bcf95bb93f583a7483129c"),
];
const AWGENEALOGY_V3_HASHES: &[(&str, &str)] = &[
    ("action.php", "7e7e5068db229baa78c04357c5c725f112726fc909303a1fc2d7bc9a8f809e7f"),
    ("helper.php", "67f0b3d10387c40fee241966f8c6d6d1d5a910adbeb1e5e7db6fcde88365c0fa"),
    ("plugin.info.txt", "3b1e4c6623f5b42e3efb29e9f4eb5017cbd63de78f6ce94b3cbd78f8cc233f9a"),
    ("print.css", "5a93c3b14d911850c3e299b241310f644e7684dab1cbfa60f6af8b239013a471"),
    ("script.js", "b9e548ff885eb081945b3b0f2fd75572bae7d9fa27576ded6f412f77983d753a"),
    ("style.css", "bf583915552544f8c6dbdb5d738e274b204f54b1a4d6425d70032e8660f42ec0"),
    ("syntax/directory.php", "2a4d51bbbc257f9fd590408ffb266a62e80c6a41cdfabcee9df6f8126f630ed9"),
    ("syntax/familygroup.php", "2b3f3c283030574f90461df4bef0eb0206721b9dfa0545d3b827a23c165793c6"),
    ("syntax/newperson.php", "993b11a382f96057887e3803ea6bba03395b6cc17cb8599342ef343b4e93d442"),
    ("syntax/pedigree.php", "3bc5624907770ac3b080655c9d2fcf79d2d05bfad85e645ed96ead9b99da0705"),
    ("syntax/person.php", "8c895dd3a703c52c78ec12cce0d45da157e1ef5121ced2a8769e2ef8d2736e90"),
];
const AWGENEALOGY_PRE_FAMILY_GROUP_HASHES: &[(&str, &str)] = &[
    ("action.php", "22035636a43cf5a585a64031c63dff2b941777b796207d992d82c8dae75ff29b"),
    ("helper.php", "ac6238f91dc6777119dcc8be1fc7f63c57bcd93d46847a5ff345b10cfe53b8cd"),
    ("plugin.info.txt", "c84c6073d34e10e05988d75f0df809e34b0df6f832d91b999d24eddfcc0fca2e"),
    ("print.css", "aeae3e2a0710ce60cb2afb8183d62c88555b34adb32ef49e0c0ce6741393d37d"),
    ("script.js", "42e6d2294ae19455285ca49ee619a62fc8b838079a7430d6ec2353c5c2bf3a69"),
    ("style.css", "5a05fce1e7b58c7ad8599f613819f4a25a33244ff983e88fd704f7e5d7b965af"),
    ("syntax/pedigree.php", "2f3a7ca2c5ea4c00c65929b31c28a41002981ce2d82967419543679ec2cf5aa6"),
    ("syntax/person.php", "f3d1dc8286a2c65d71ef1e252c36ff277960446216bcf95bb93f583a7483129c"),
];

fn plugin_matches_hash_snapshot(dst: &Path, actual: &[PathBuf], snapshot: &[(&str, &str)]) -> bool {
    let mut expected: Vec<PathBuf> = snapshot.iter().map(|(path, _)| PathBuf::from(*path)).collect();
    expected.sort();
    if actual != expected.as_slice() {
        return false;
    }
    snapshot.iter().all(|(path, hash)| {
        snapshot::sha256_file(&dst.join(path))
            .map(|actual| actual == *hash)
            .unwrap_or(false)
    })
}

fn collect_plugin_files(root: &Path, dir: &Path, files: &mut Vec<PathBuf>) -> bool {
    let Ok(entries) = fs::read_dir(dir) else { return false };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else { return false };
        if kind.is_dir() {
            if !collect_plugin_files(root, &path, files) { return false; }
        } else if kind.is_file() {
            let Ok(rel) = path.strip_prefix(root) else { return false };
            files.push(rel.to_path_buf());
        } else {
            return false;
        }
    }
    true
}

/// Upgrade only exact snapshots of bundled ArchiveWiki Genealogy versions; user-edited plugins are left intact.
fn upgrade_unmodified_awgenealogy(src: &Path, dst: &Path, target_root: &Path) {
    if !dst.is_dir() { return; }
    let mut actual = Vec::new();
    if !collect_plugin_files(dst, dst, &mut actual) { return; }
    actual.sort();
    let mut expected: Vec<PathBuf> = AWGENEALOGY_V1_FILES.iter().map(|(p, _)| PathBuf::from(*p)).collect();
    expected.sort();
    let mut matches_v1 = actual == expected;
    if matches_v1 {
        for (rel, shipped) in AWGENEALOGY_V1_FILES {
            if fs::read_to_string(dst.join(*rel)).map(|s| s != *shipped).unwrap_or(true) {
                matches_v1 = false;
                break;
            }
        }
    }
    let matches_known_release = plugin_matches_hash_snapshot(dst, &actual, AWGENEALOGY_V3_HASHES)
        || plugin_matches_hash_snapshot(dst, &actual, AWGENEALOGY_V2_HASHES)
        || plugin_matches_hash_snapshot(dst, &actual, AWGENEALOGY_PRE_FAMILY_GROUP_HASHES);
    if !matches_v1 && !matches_known_release {
        return;
    }

    let stage = target_root.join(format!(".awgenealogy-update-{}", std::process::id()));
    let backup = target_root.join(format!(".awgenealogy-previous-{}", std::process::id()));
    if stage.exists() || backup.exists() { return; }
    if copy_tree(src, &stage, Path::new(""), &[]).is_err() {
        let _ = fs::remove_dir_all(&stage);
        return;
    }
    if fs::rename(dst, &backup).is_err() {
        let _ = fs::remove_dir_all(&stage);
        return;
    }
    if fs::rename(&stage, dst).is_err() {
        let _ = fs::rename(&backup, dst);
        let _ = fs::remove_dir_all(&stage);
        return;
    }
    let _ = fs::remove_dir_all(backup);
}

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
            if name == "awgenealogy" {
                upgrade_unmodified_awgenealogy(&e.path(), &target_root.join(&name), &target_root);
            }
            continue;
        }
        let dst = target_root.join(&name);
        if !dst.exists() && copy_tree(&e.path(), &dst, Path::new(""), &[]).is_err() {
            let _ = fs::remove_dir_all(&dst);
            continue;
        }
        if name == "awgenealogy" {
            upgrade_unmodified_awgenealogy(&e.path(), &dst, &target_root);
        }
        offered.push(name);
        changed = true;
    }
    if changed {
        let _ = fs::write(&offered_file, offered.join("\n"));
    }
}

/// Map the two Journal landing-page links to the app's existing quick-capture actions.
fn journal_action(url: &str) -> Option<UserEvent> {
    let (_, query) = url.split_once('?')?;
    let query = query.split('#').next().unwrap_or(query);
    let id = query.split('&').find_map(|part| {
        let (key, value) = part.split_once('=')?;
        (key == "id").then_some(value)
    })?;
    let id = id.replace("%3A", ":").replace("%3a", ":");
    match id.as_str() {
        "archivewiki:action_today_journal" => Some(UserEvent::TodayJournal),
        "archivewiki:action_new_note" => Some(UserEvent::QuickNote),
        _ => None,
    }
}

/// Setup wizard until DokuWiki has been configured, the wiki itself afterwards.
fn start_url(l: &Layout, origin: &str) -> String {
    // The secret in the first URL makes the server set a session cookie for this window (see router.php).
    wiki_url_with_session_token(
        origin,
        initial_wiki_route(l.wiki.join("conf").join("local.php").exists(), false),
        &l.token,
    )
}

fn login_url(l: &Layout, origin: &str) -> String {
    wiki_url_with_session_token(
        origin,
        initial_wiki_route(l.wiki.join("conf").join("local.php").exists(), true),
        &l.token,
    )
}

fn wiki_url_with_session_token(origin: &str, route: &str, token: &str) -> String {
    let separator = if route.contains('?') { '&' } else { '?' };
    format!("{origin}{route}{separator}aw={token}")
}

fn startup_action(url: &str, startup_page: bool) -> Option<&str> {
    if !startup_page {
        return None;
    }
    let action = url.strip_prefix("archivewiki://startup/")?;
    matches!(action, "login" | "create" | "import" | "continue" | "quit").then_some(action)
}

fn initial_wiki_route(configured: bool, login: bool) -> &'static str {
    match (configured, login) {
        (false, _) => "/install.php",
        (true, true) => "/doku.php?do=login",
        (true, false) => "/doku.php",
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
    let root = if file == "update-skipped" { &l.profile_root } else { &l.support };
    fs::read_to_string(root.join(file)).map(|s| s.trim() == version).unwrap_or(false)
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
            let _ = fs::write(l.profile_root.join("update-skipped"), &rk);
            return Outcome::Offered;
        }
        Choice::Update => {}
    }
    match prepare_update(l, &installed) {
        Ok(staging) => {
            let _ = proxy.send_event(UserEvent::ApplyUpdate { profile_id: l.profile_id.clone(), staging, version: remote });
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
            if v.0 == major && v.1 == minor && best.is_none_or(|b| v > b) {
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
    let _ = fs::remove_dir_all(&l.php_updates);
    fs::create_dir_all(&l.php_updates)?;
    let tgz = l.php_updates.join("php.tgz");
    curl_file(&php_download_url(new_ver), &tgz)?;
    let digest = match std::env::var("ARCHIVEWIKI_PHP_SHA256") {
        Ok(expected) => expected,
        Err(_) => release_asset_sha256(
            PHP_REPO,
            &format!("php-{new_ver}"),
            &format!("php-{new_ver}-cli-macos-aarch64.tar.gz"),
        )?,
    };
    snapshot::verify_sha256(&tgz, &digest)
        .context("the downloaded PHP package failed integrity verification")?;
    let stage = l.php_updates.join("php-stage");
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
    let _ = fs::remove_dir_all(&l.php_updates);
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
    let version = curl_text(VERSION_URL)?;
    let version = version_key(&version);
    if !valid_version(&version) || !is_newer(&version, installed) {
        bail!("The advertised DokuWiki release is not a newer valid version.");
    }
    let tag = format!("release-{version}");
    let asset_name = format!("dokuwiki-{version}.tgz");
    let digest = match std::env::var("ARCHIVEWIKI_DOKUWIKI_SHA256") {
        Ok(expected) => expected,
        Err(_) => release_asset_sha256("dokuwiki/dokuwiki", &tag, &asset_name)?,
    };
    let url = format!("https://github.com/dokuwiki/dokuwiki/releases/download/{tag}/{asset_name}");
    curl_file(&url, &tgz)?;
    snapshot::verify_sha256(&tgz, &digest)
        .context("the downloaded DokuWiki package failed integrity verification")?;
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
    if version_key(&got) != version {
        bail!(
            "The archive reports DokuWiki {} but release metadata advertised {version}.",
            version_key(&got)
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
        let mut command = Command::new(&l.php);
        command.args([
            "-n",
            "-d", "memory_limit=512M",
            "-d", "upload_max_filesize=128M",
            "-d", "post_max_size=128M",
            "-d", "max_execution_time=120",
            "-d", "display_errors=0",
            "-d", "log_errors=1",
        ])
            .arg("-d")
            .arg(format!("date.timezone={}", detect_tz()));
        if let Some(preload) = &l.session_preload {
            command.arg("-d").arg(format!("auto_prepend_file={}", preload.display()));
        }
        let child = command
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

fn app_path<'a>(origin: &str, url: &'a str) -> Option<&'a str> {
    url.strip_prefix(origin)
        .filter(|rest| rest.is_empty() || rest.starts_with('/') || rest.starts_with('?') || rest.starts_with('#'))
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

fn backup_root() -> Result<PathBuf> {
    Ok(dirs::document_dir().ok_or_else(|| anyhow!("Cannot locate ~/Documents"))?.join("ArchiveWiki Backups"))
}

fn profile_folder_name_for(name: &str, id: &str) -> String {
    format!("{name} [{id}]")
}

fn profile_folder_name(l: &Layout) -> String {
    profile_folder_name_for(&l.profile_name, &l.profile_id)
}

fn backup_dir(l: &Layout) -> Result<PathBuf> {
    let root = backup_root()?;
    if l.profile_id == DEFAULT_WIKI_ID {
        Ok(root)
    } else {
        Ok(root.join(profile_folder_name(l)))
    }
}

fn backup(l: &Layout, prefix: &str) -> Result<PathBuf> {
    let _guard = WIKI_IO_LOCK.lock().map_err(|_| anyhow!("Wiki file operation lock is poisoned"))?;
    backup_unlocked(l, prefix)
}

fn backup_unlocked(l: &Layout, prefix: &str) -> Result<PathBuf> {
    let dir = backup_dir(l)?;
    fs::create_dir_all(&dir)?;
    let ts = Command::new("date").arg("+%Y%m%d-%H%M%S").output()?;
    let ts = String::from_utf8_lossy(&ts.stdout).trim().to_string();
    let out = unique_path(dir.join(format!("{prefix}-{ts}.zip")));
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
    !l.profile_root.join("backup-auto-off").exists()
}

fn auto_backup_due(l: &Layout) -> bool {
    fs::read_to_string(l.profile_root.join("last-auto-backup"))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok())
        .map(|t| now_secs().saturating_sub(t) >= AUTO_BACKUP_INTERVAL_SECS)
        .unwrap_or(true)
}

/// Keep only the newest `keep` automatic backups (manual and pre-update/pre-restore backups are never deleted).
fn prune_auto_backups(l: &Layout, keep: usize) {
    let Ok(dir) = backup_dir(l) else { return };
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
        if let Ok(profiles) = wiki_profiles(&l.support) {
            for profile in profiles {
                let profile_layout = l.for_profile(&profile);
                if auto_backup_enabled(&profile_layout)
                    && auto_backup_due(&profile_layout)
                    && backup(&profile_layout, "auto-backup").is_ok()
                {
                    let _ = fs::write(profile_layout.profile_root.join("last-auto-backup"), now_secs().to_string());
                    prune_auto_backups(&profile_layout, AUTO_BACKUP_KEEP);
                }
            }
        }
        thread::sleep(Duration::from_secs(60 * 60));
    }
}

/// Background thread: let the user pick a backup, validate it, confirm, then hand it to the main thread.
fn restore_flow(l: &Layout, proxy: &EventLoopProxy<UserEvent>) -> Result<()> {
    let Some(zip) = pick_zip("Choose a backup to restore", Some(backup_dir(l)?))? else { return Ok(()) };
    if let Err(e) = validate_backup_zip(&zip) {
        alert(&format!("That file can't be restored.\n\n{e:#}"));
        return Ok(());
    }
    let name = zip.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let msg = format!(
        "Restore from \"{name}\"?\n\nArchiveWiki will first save a safety copy of the current archive. Then it will replace your pages, uploaded files, and settings with those in this backup. Any changes made since the backup was created will be removed."
    );
    let script = format!(
        "display dialog \"{}\" with title \"{APP_NAME}\" buttons {{\"Cancel\", \"Restore\"}} default button \"Cancel\" cancel button \"Cancel\" with icon caution",
        esc(&msg)
    );
    let out = Command::new("osascript").args(["-e", &script]).output().context("could not confirm the restore")?;
    if out.status.success() && String::from_utf8_lossy(&out.stdout).contains("Restore") {
        proxy
            .send_event(UserEvent::RestoreBackup { profile_id: l.profile_id.clone(), zip })
            .map_err(|e| anyhow!("could not start the restore: {e}"))?;
    }
    Ok(())
}

/// Main thread, server stopped. Stages and validates the archive before replacing live files.
fn restore_backup(l: &Layout, zip: &Path, save_current: bool) -> Result<()> {
    let _guard = WIKI_IO_LOCK.lock().map_err(|_| anyhow!("Wiki file operation lock is poisoned"))?;
    let entries = inspect_archive(zip)?;
    let stage = l.profile_root.join("restore-stage");
    let result = (|| -> Result<()> {
        extract_archive(zip, &stage, &entries)?;
        for top in ["data", "conf"] {
            if !stage.join(top).is_dir() {
                bail!("The backup is missing its {top} folder.");
            }
        }
        let _ = fs::remove_file(stage.join(snapshot::MANIFEST));
        let _ = fs::remove_file(stage.join("SNAPSHOT.txt"));
        if save_current {
            backup_unlocked(l, "pre-restore").context("could not save your current wiki first")?;
        }
        install_staged_restore(&l.wiki, &stage, &l.profile_root.join("restore-previous"))?;
        let data = l.wiki.join("data");
        for d in ["cache", "tmp", "locks", "log", "index", "meta", "attic", "media", "media_attic", "media_meta", "pages"] {
            let _ = fs::create_dir_all(data.join(d));
        }
        Ok(())
    })();
    let _ = fs::remove_dir_all(&stage);
    result
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
        "(function(){{var t='{default_mode}';\
         function a(){{document.documentElement.setAttribute('data-aw-theme',t);}}\
         if(document.documentElement){{a();}}else{{document.addEventListener('DOMContentLoaded',a);}}}})();"
    )
}

fn app_ui_script(l: &Layout) -> String {
    let key = recent_pages_storage_key(&l.profile_id);
    APP_UI_SCRIPT.replace("archivewiki.recent-pages", &key)
}

fn recent_pages_storage_key(profile_id: &str) -> String {
    if profile_id == DEFAULT_WIKI_ID {
        "archivewiki.recent-pages".to_owned()
    } else {
        format!("archivewiki.recent-pages.{profile_id}")
    }
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

fn snapshot_root() -> Result<PathBuf> {
    Ok(dirs::document_dir().ok_or_else(|| anyhow!("Cannot locate ~/Documents"))?.join("ArchiveWiki Snapshots"))
}

fn snapshot_dir(l: &Layout) -> Result<PathBuf> {
    let root = snapshot_root()?;
    if l.profile_id == DEFAULT_WIKI_ID {
        Ok(root)
    } else {
        Ok(root.join(profile_folder_name(l)))
    }
}

/// A snapshot is a restorable zip of data/ and conf/ plus MANIFEST.sha256 (a SHA-256 for every file) and
/// SNAPSHOT.txt, with the zip's own checksum saved beside it. Files are cloned first so the manifest and the zip agree.
fn create_snapshot(l: &Layout) -> Result<(PathBuf, usize)> {
    let _guard = WIKI_IO_LOCK.lock().map_err(|_| anyhow!("Wiki file operation lock is poisoned"))?;
    let dir = snapshot_dir(l)?;
    fs::create_dir_all(&dir)?;
    let stage = l.profile_root.join("snapshot-staging");
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
        let requested_name = format!("ArchiveWiki-snapshot-{}.zip", stamp("+%Y%m%d-%H%M%S"));
        let out = unique_path(dir.join(&requested_name));
        let name = out.file_name().ok_or_else(|| anyhow!("Snapshot path has no file name"))?.to_string_lossy().into_owned();
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
    let entries = inspect_archive(zip)?;
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
    let tmp = l.profile_root.join("snapshot-verify");
    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    let res = (|| -> Result<(bool, String)> {
        extract_archive(zip, &tmp, &entries).context("The zip couldn't be safely unpacked")?;
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

fn pick_zip(prompt: &str, start: Option<PathBuf>) -> Result<Option<PathBuf>> {
    let loc = start
        .filter(|d| d.exists())
        .map(|d| format!(" default location (POSIX file \"{}\")", esc(&d.to_string_lossy())))
        .unwrap_or_default();
    let script = format!("POSIX path of (choose file with prompt \"{}\" of type {{\"zip\"}}{loc})", esc(prompt));
    let out = Command::new("osascript").args(["-e", &script]).output().context("could not open the file chooser")?;
    if !out.status.success() {
        return Ok(None); // cancelled
    }
    Ok(Some(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim())))
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
    let start = match snapshot_dir(l) {
        Ok(dir) => Some(dir),
        Err(e) => {
            alert(&format!("The snapshot folder could not be located.\n\n{e:#}"));
            return;
        }
    };
    let zip = match pick_zip("Choose a snapshot to verify", start) {
        Ok(Some(zip)) => zip,
        Ok(None) => return,
        Err(e) => {
            alert(&format!("The snapshot file chooser could not be opened.\n\n{e:#}"));
            return;
        }
    };
    match verify_snapshot_zip(l, &zip) {
        Ok((true, t)) => info(&format!("Snapshot verified.\n\n{t}")),
        Ok((false, t)) => alert(&format!("Problem found.\n\n{t}")),
        Err(e) => alert(&format!("The snapshot could not be checked.\n\n{e:#}")),
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn prompt_wiki_name(importing: bool) -> Result<Option<String>> {
    let prompt = if importing { "Name this imported wiki:" } else { "Name your new wiki:" };
    let script = format!(
        "text returned of (display dialog \"{}\" with title \"{APP_NAME}\" default answer \"\" buttons {{\"Cancel\", \"Continue\"}} default button \"Continue\" cancel button \"Cancel\")",
        esc(prompt)
    );
    let out = Command::new("osascript").args(["-e", &script]).output().context("could not open the wiki name prompt")?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

fn prompt_wiki_rename(profile: &WikiProfile) -> Result<Option<String>> {
    let script = format!(
        "text returned of (display dialog \"Choose a new name for this wiki:\" with title \"{APP_NAME}\" default answer \"{}\" buttons {{\"Cancel\", \"Save\"}} default button \"Save\" cancel button \"Cancel\")",
        esc(&profile.name)
    );
    let out = Command::new("osascript").args(["-e", &script]).output().context("could not open the wiki rename prompt")?;
    if !out.status.success() {
        return Ok(None);
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).trim().to_string()))
}

fn confirm_delete_wiki(profile: &WikiProfile) -> Result<bool> {
    let message = format!(
        "Permanently delete \"{}\" and its pages, settings, and profile files?\n\nExisting backup archives in Documents will be kept. This action cannot be undone.",
        profile.name
    );
    let script = format!(
        "display dialog \"{}\" with title \"{APP_NAME}\" buttons {{\"Delete Wiki\", \"Cancel\"}} default button \"Cancel\" cancel button \"Cancel\" with icon caution",
        esc(&message)
    );
    let out = Command::new("osascript").args(["-e", &script]).output().context("could not confirm wiki deletion")?;
    if !out.status.success() {
        if out.status.code() == Some(1) {
            return Ok(false);
        }
        bail!("The wiki deletion confirmation could not be displayed.");
    }
    Ok(String::from_utf8_lossy(&out.stdout).contains("button returned:Delete Wiki"))
}

fn create_wiki_flow(l: &Layout, proxy: &EventLoopProxy<UserEvent>, importing: bool, startup: bool) -> Result<()> {
    let Some(name) = prompt_wiki_name(importing)? else { return Ok(()) };
    let import = if importing {
        let Some(zip) = pick_zip("Choose an ArchiveWiki backup to import", Some(backup_root()?))? else { return Ok(()) };
        Some(zip)
    } else {
        None
    };
    let (l, proxy) = (l.clone(), proxy.clone());
    thread::spawn(move || {
        let result = create_wiki_profile(&l, &name, import.as_deref()).map_err(|e| format!("{e:#}"));
        let event = if startup { UserEvent::StartupWikiCreated(result) } else { UserEvent::WikiCreated(result) };
        let _ = proxy.send_event(event);
    });
    Ok(())
}

fn choose_existing_wiki_flow(l: &Layout, proxy: &EventLoopProxy<UserEvent>) {
    let l = l.clone();
    let proxy = proxy.clone();
    thread::spawn(move || {
        let result = choose_wiki_profile(&l).map_err(|e| format!("{e:#}"));
        let _ = proxy.send_event(UserEvent::StartupProfileSelected(result));
    });
}

fn choose_wiki_profile(l: &Layout) -> Result<Option<WikiProfile>> {
    let profiles = wiki_profiles(&l.support)?
        .into_iter()
        .filter(|profile| profile.wiki.join("doku.php").is_file())
        .collect::<Vec<_>>();
    if profiles.is_empty() {
        return Ok(None);
    }
    let choices = profiles.iter().map(|profile| format!("\"{}\"", esc(&profile.name))).collect::<Vec<_>>().join(", ");
    let script = format!(
        "choose from list {{{choices}}} with prompt \"Choose a wiki to open\" with title \"{APP_NAME}\""
    );
    let out = Command::new("osascript").args(["-e", &script]).output().context("could not open the wiki chooser")?;
    if !out.status.success() {
        return Ok(None);
    }
    let chosen = String::from_utf8_lossy(&out.stdout).trim().to_string();
    profiles
        .into_iter()
        .find(|profile| profile.name == chosen)
        .map(Some)
        .ok_or_else(|| anyhow!("The selected wiki could not be found in the profile list."))
}

fn relaunch_application(open_selected: bool, login: bool) -> Result<()> {
    let executable = std::env::current_exe()?;
    let mut args = Vec::new();
    if open_selected {
        args.push("--open-selected");
    }
    if login {
        args.push("--login");
    }
    if let Some(bundle) = executable.ancestors().find(|path| path.extension().is_some_and(|ext| ext == "app")) {
        let status = Command::new("/usr/bin/open").args(["-n"]).arg(bundle).arg("--args").args(args).status()?;
        if !status.success() {
            bail!("macOS could not reopen ArchiveWiki");
        }
    } else {
        Command::new(executable).args(args).spawn()?;
    }
    Ok(())
}

fn confirm_wiki_switch(name: &str) -> Result<bool> {
    let message = format!(
        "Switch to \"{name}\"?\n\nArchiveWiki will reopen on that wiki. Save any edits first."
    );
    let script = format!(
        "display dialog \"{}\" with title \"{APP_NAME}\" buttons {{\"Cancel\", \"Switch\"}} default button \"Cancel\" cancel button \"Cancel\" with icon caution",
        esc(&message)
    );
    Ok(Command::new("osascript").args(["-e", &script]).status().context("could not confirm the wiki switch")?.success())
}

fn reopen_profile(
    l: &Layout,
    profile: &WikiProfile,
    login: bool,
    port: u16,
    server: &mut Server,
    control_flow: &mut ControlFlow,
) -> Result<bool> {
    if profile.id == l.profile_id {
        return Ok(false);
    }
    fs::write(l.support.join("active-wiki"), &profile.id)?;
    server.stop();
    if let Err(launch_error) = relaunch_application(true, login) {
        let selection_error = fs::write(l.support.join("active-wiki"), &l.profile_id).err();
        match start_checked(l, port) {
            Ok(restored) => {
                *server = restored;
                return Err(match selection_error {
                    Some(selection_error) => anyhow!(
                        "Could not switch wikis, and the saved selection could not be reset ({selection_error}). The current wiki remains open.\n\n{launch_error:#}"
                    ),
                    None => anyhow!("Could not switch wikis; the current wiki remains open.\n\n{launch_error:#}"),
                });
            }
            Err(restart_error) => {
                *control_flow = ControlFlow::Exit;
                return Err(match selection_error {
                    Some(selection_error) => anyhow!(
                        "Could not switch wikis ({launch_error:#}), reset the selection ({selection_error}), or restart the current wiki server ({restart_error:#})."
                    ),
                    None => anyhow!(
                        "Could not switch wikis ({launch_error:#}) or restart the current wiki server ({restart_error:#})."
                    ),
                });
            }
        }
    }
    *control_flow = ControlFlow::Exit;
    Ok(true)
}

fn switch_profile(
    l: &Layout,
    profile: &WikiProfile,
    port: u16,
    server: &mut Server,
    control_flow: &mut ControlFlow,
) -> Result<bool> {
    if profile.id == l.profile_id || !confirm_wiki_switch(&profile.name)? {
        return Ok(false);
    }
    reopen_profile(l, profile, false, port, server, control_flow)
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
    let acc = |m: Modifiers, c: Code| Some(Accelerator::new(m, c));
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
    let wikis = Submenu::new("Manage Wikis", true);
    wikis.append_items(&[
        &MenuItem::with_id("newwiki", "New Wiki…", true, None),
        &MenuItem::with_id("importwiki", "Import Backup as New Wiki…", true, None),
        &MenuItem::with_id("switchwiki", "Switch Wiki…", true, None),
        &MenuItem::with_id("renamewiki", "Rename Wiki…", true, None),
        &MenuItem::with_id("deletewiki", "Delete Wiki…", true, None),
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("wikisfolder", "Show Wikis Folder in Finder", true, None),
    ])?;
    let snapshots = Submenu::new("Snapshots & Verification (Advanced)", true);
    snapshots.append_items(&[
        &MenuItem::with_id("snapshot", "Create Archive Snapshot…", true, None),
        &MenuItem::with_id("verifysnap", "Verify Snapshot…", true, None),
    ])?;
    let backups = Submenu::new("Back Up & Restore", true);
    backups.append_items(&[
        &MenuItem::with_id("backup", "Back Up Now", true, acc(Modifiers::META | Modifiers::SHIFT, Code::KeyB)),
        &backup_item,
        &MenuItem::with_id("restore", "Restore from Backup…", true, None),
        &PredefinedMenuItem::separator(),
        &snapshots,
    ])?;
    file.append_items(&[
        &wikis,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("newnote", "New Note", true, acc(Modifiers::META | Modifiers::SHIFT, Code::KeyN)),
        &MenuItem::with_id("todayjournal", "Today's Journal Entry", true, acc(Modifiers::META | Modifiers::SHIFT, Code::KeyJ)),
        &hotkey_item,
        &PredefinedMenuItem::separator(),
        &MenuItem::with_id("print", "Print…", true, acc(Modifiers::META, Code::KeyP)),
        &backups,
        &PredefinedMenuItem::separator(),
        &lock_require,
        &idle_submenu,
        &MenuItem::with_id("locknow", "Lock Now", true, acc(Modifiers::META | Modifiers::SHIFT, Code::KeyL)),
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
        &MenuItem::with_id("search", "Search Archive", true, acc(Modifiers::META, Code::KeyK)),
        &MenuItem::with_id("home", "Home", true, acc(Modifiers::META | Modifiers::SHIFT, Code::KeyH)),
        &MenuItem::with_id("back", "Back", true, acc(Modifiers::META, Code::BracketLeft)),
        &MenuItem::with_id("forward", "Forward", true, acc(Modifiers::META, Code::BracketRight)),
        &MenuItem::with_id("reload", "Reload", true, acc(Modifiers::META, Code::KeyR)),
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
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    let open_selected = args.iter().any(|arg| arg == "--open-selected");
    let login = open_selected && args.iter().any(|arg| arg == "--login");
    let mut layout = Layout::discover()?;
    if open_selected {
        let profile = active_wiki_profile(&layout.support)?;
        layout = layout.for_profile(&profile);
    }
    kill_stale(&layout.pidfile);
    sync_php(&layout.bundle_php, &layout.php)?;
    install_auth_helper(&layout);
    sync_wiki(&layout.bundle_wiki, &layout.wiki)?;
    install_plugins(&layout);
    install_profile_session(&layout)?;
    fs::write(&layout.router, ROUTER_PHP)?;

    let port = pick_port();
    let mut server = Server::start(&layout, port)?;
    wait_ready(port, &mut server.child)?;

    let origin = format!("http://127.0.0.1:{port}");

    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let menu_proxy = event_loop.create_proxy();
    let update_proxy = event_loop.create_proxy();
    let hotkey_proxy = event_loop.create_proxy();
    let action_proxy = event_loop.create_proxy();
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
    MenuEvent::set_event_handler(Some(Arc::new(move |e| {
        let _ = menu_proxy.send_event(UserEvent::Menu(e));
    })));
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
    let popup_origin = origin.clone();
    let startup_page = !open_selected;
    let initialization_script = if open_selected {
        format!("{}{}", appearance_script(&appearance_mode(&layout)), app_ui_script(&layout))
    } else {
        String::new()
    };
    let webview_builder = WebViewBuilder::new()
        .with_initialization_script(&initialization_script)
        .with_navigation_handler(move |u: String| {
            if let Some(action) = startup_action(&u, startup_page) {
                let _ = action_proxy.send_event(UserEvent::StartupAction(action.to_owned()));
                false
            } else if let Some(path) = app_path(&nav_origin, &u) {
                if let Some(action) = journal_action(path) {
                    let _ = action_proxy.send_event(action);
                    false
                } else {
                    true
                }
            } else if u.starts_with("about:") || u.starts_with("blob:") || u.starts_with("data:") {
                true
            } else {
                open_external(&u);
                false
            }
        })
        .with_new_window_req_handler(move |u: String, _features| {
            if app_path(&popup_origin, &u).is_some() {
                NewWindowResponse::Allow
            } else {
                open_external(&u);
                NewWindowResponse::Deny
            }
        })
        .with_download_started_handler(|_url: String, dest: &mut PathBuf| {
            if let (Some(dir), Some(name)) = (dirs::download_dir(), dest.file_name().map(|n| n.to_owned())) {
                *dest = unique_path(dir.join(name));
            }
            true
        })
        .with_devtools(cfg!(debug_assertions));
    let webview_builder = if open_selected {
        webview_builder.with_url(if login { login_url(&layout, &origin) } else { start_url(&layout, &origin) })
    } else {
        webview_builder.with_html(STARTUP_HTML)
    };
    let webview = webview_builder
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
            Event::UserEvent(UserEvent::StartupAction(action)) => match action.as_str() {
                "continue" => {
                    let _ = webview.load_url(&start_url(&layout, &origin));
                }
                "login" => choose_existing_wiki_flow(&layout, &update_proxy),
                "create" => {
                    if let Err(e) = create_wiki_flow(&layout, &update_proxy, false, true) {
                        alert(&format!("The new wiki could not be started.\n\n{e:#}"));
                    }
                }
                "import" => {
                    if let Err(e) = create_wiki_flow(&layout, &update_proxy, true, true) {
                        alert(&format!("The wiki import could not be started.\n\n{e:#}"));
                    }
                }
                "quit" => {
                    server.stop();
                    *control_flow = ControlFlow::Exit;
                }
                _ => alert("That startup option isn't recognized. Please restart ArchiveWiki."),
            },
            Event::UserEvent(UserEvent::StartupProfileSelected(result)) => match result {
                Ok(Some(profile)) if profile.id == layout.profile_id => {
                    let _ = webview.load_url(&login_url(&layout, &origin));
                }
                Ok(Some(profile)) => {
                    if let Err(e) = reopen_profile(&layout, &profile, true, port, &mut server, control_flow) {
                        alert(&format!("The selected wiki could not be opened.\n\n{e:#}"));
                    }
                }
                Ok(None) => {}
                Err(e) => alert(&format!("The existing wiki list could not be opened.\n\n{e}")),
            },
            Event::UserEvent(UserEvent::StartupWikiCreated(result)) => match result {
                Ok(profile) => {
                    if let Err(e) = reopen_profile(&layout, &profile, false, port, &mut server, control_flow) {
                        alert(&format!("The wiki was created, but ArchiveWiki couldn't open it.\n\n{e:#}"));
                    }
                }
                Err(e) => alert(&format!("The wiki could not be created.\n\n{e}")),
            },
            Event::UserEvent(UserEvent::ApplyUpdate { profile_id, staging, version }) => {
                if profile_id != layout.profile_id {
                    alert("The wiki changed before this update could be applied. Check for updates again from the active wiki.");
                    return;
                }
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
                    Ok(()) => info(&format!("Updated to DokuWiki {version}.\n\nA pre-update backup was saved in this wiki's backup folder.")),
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
            Event::UserEvent(UserEvent::RestoreBackup { profile_id, zip }) => {
                if profile_id != layout.profile_id {
                    alert("The active wiki changed before this restore could begin. Please choose the backup again.");
                    return;
                }
                server.stop();
                let mut res = restore_backup(&layout, &zip, true);
                match start_checked(&layout, port) {
                    Ok(s) => server = s,
                    Err(start_error) if res.is_ok() => {
                        let previous = layout.profile_root.join("restore-previous");
                        let failed = layout.profile_root.join("restore-failed");
                        if let Err(rollback_error) = install_staged_restore(&layout.wiki, &previous, &failed) {
                            alert(&format!("The restored wiki would not start ({start_error:#}), and its previous files could not be put back ({rollback_error:#})."));
                            *control_flow = ControlFlow::Exit;
                            return;
                        }
                        res = Err(anyhow!("The restored wiki did not start ({start_error:#}); the previous pages and settings were restored."));
                        match start_checked(&layout, port) {
                            Ok(s) => server = s,
                            Err(recovery_error) => {
                                alert(&format!("The previous wiki files were restored, but the server could not restart: {recovery_error:#}"));
                                *control_flow = ControlFlow::Exit;
                                return;
                            }
                        }
                    }
                    Err(e) => {
                        alert(&format!("Could not restart the wiki server: {e:#}"));
                        *control_flow = ControlFlow::Exit;
                        return;
                    }
                }
                let _ = webview.load_url(&start_url(&layout, &origin));
                thread::spawn(move || match res {
                    Ok(()) => info("Backup restored. Your previous wiki was saved as a pre-restore backup in this wiki's backup folder."),
                    Err(e) => alert(&format!("The restore didn't complete.\n\n{e:#}")),
                });
            }
            Event::UserEvent(UserEvent::WikiCreated(result)) => match result {
                Ok(profile) => {
                    match switch_profile(&layout, &profile, port, &mut server, control_flow) {
                        Ok(true) => {}
                        Ok(false) => info(&format!(
                            "\"{}\" was created. You can open it later from File → Manage Wikis → Switch Wiki.",
                            profile.name
                        )),
                        Err(e) => alert(&format!("The wiki was created, but ArchiveWiki couldn't switch to it.\n\n{e:#}")),
                    }
                }
                Err(e) => alert(&format!("The wiki could not be created.\n\n{e}")),
            },
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
                "folder" => { let _ = Command::new("open").arg(&layout.wiki).spawn(); }
                "newwiki" => {
                    if let Err(e) = create_wiki_flow(&layout, &update_proxy, false, false) {
                        alert(&format!("The new wiki could not be started.\n\n{e:#}"));
                    }
                }
                "importwiki" => {
                    if let Err(e) = create_wiki_flow(&layout, &update_proxy, true, false) {
                        alert(&format!("The wiki import could not be started.\n\n{e:#}"));
                    }
                }
                "switchwiki" => {
                    match choose_wiki_profile(&layout) {
                        Ok(Some(profile)) => {
                            if let Err(e) = switch_profile(&layout, &profile, port, &mut server, control_flow) {
                                alert(&format!("{e:#}"));
                            }
                        }
                        Ok(None) => {}
                        Err(e) => alert(&format!("The wiki chooser could not be opened.\n\n{e:#}")),
                    }
                }
                "renamewiki" => match choose_wiki_profile(&layout) {
                    Ok(Some(profile)) => match prompt_wiki_rename(&profile) {
                        Ok(Some(name)) => {
                            let backups_root = if profile.id == DEFAULT_WIKI_ID {
                                Ok(None)
                            } else {
                                backup_root().map(Some)
                            };
                            match backups_root {
                                Ok(backups_root) => match rename_wiki_profile(
                                    &layout.support,
                                    &profile.id,
                                    &name,
                                    backups_root.as_deref(),
                                ) {
                                    Ok(renamed) => {
                                        if renamed.id == layout.profile_id {
                                            layout.profile_name = renamed.name.clone();
                                        }
                                        info(&format!("This wiki is now named \"{}\".", renamed.name));
                                    }
                                    Err(e) => alert(&format!("The wiki could not be renamed.\n\n{e:#}")),
                                },
                                Err(e) => alert(&format!("The wiki backup folder could not be located.\n\n{e:#}")),
                            }
                        }
                        Ok(None) => {}
                        Err(e) => alert(&format!("The wiki rename prompt could not be opened.\n\n{e:#}")),
                    },
                    Ok(None) => {}
                    Err(e) => alert(&format!("The wiki chooser could not be opened.\n\n{e:#}")),
                },
                "deletewiki" => match choose_wiki_profile(&layout) {
                    Ok(Some(profile)) if profile.id == DEFAULT_WIKI_ID => {
                        alert("The built-in Default Wiki is protected and cannot be deleted.");
                    }
                    Ok(Some(profile)) if profile.id == layout.profile_id => {
                        alert("Switch to another wiki before deleting this one.");
                    }
                    Ok(Some(profile)) => match confirm_delete_wiki(&profile) {
                        Ok(true) => match delete_wiki_profile(&layout.support, &profile.id, &layout.profile_id) {
                            Ok(()) => info(&format!("\"{}\" was deleted. Its existing backup archives were kept.", profile.name)),
                            Err(e) => alert(&format!("The wiki could not be deleted.\n\n{e:#}")),
                        },
                        Ok(false) => {}
                        Err(e) => alert(&format!("The deletion confirmation could not be shown.\n\n{e:#}")),
                    },
                    Ok(None) => {}
                    Err(e) => alert(&format!("The wiki chooser could not be opened.\n\n{e:#}")),
                },
                "wikisfolder" => {
                    let _ = Command::new("open").arg(layout.support.join("wikis")).spawn();
                }
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
                        let _ = webview.evaluate_script(&format!("document.documentElement.setAttribute('data-aw-theme','{mode}');"));
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
                    let (l, p) = (layout.clone(), update_proxy.clone());
                    thread::spawn(move || {
                        if let Err(e) = restore_flow(&l, &p) {
                            alert(&format!("The restore could not be started.\n\n{e:#}"));
                        }
                    });
                }
                "autobackup" => {
                    let flag = layout.profile_root.join("backup-auto-off");
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

#[cfg(test)]
mod tests {
    use super::{
        active_wiki_profile, app_path, delete_wiki_profile, initial_wiki_route, profile_session_name,
        plugin_matches_hash_snapshot, recent_pages_storage_key, release_asset_sha256_from_json,
        rename_wiki_profile, startup_action, unique_path, validate_wiki_name, wiki_profiles,
        wiki_url_with_session_token, STARTUP_HTML,
    };

    #[test]
    fn genealogy_plugin_upgrade_snapshot_rejects_edits_and_extra_files() {
        let dir = std::env::temp_dir().join(format!("archivewiki-plugin-snapshot-{}", super::random_token()));
        std::fs::create_dir_all(&dir).unwrap();
        let plugin = dir.join("plugin.php");
        std::fs::write(&plugin, "<?php echo 'bundled';").unwrap();
        let digest = super::snapshot::sha256_file(&plugin).unwrap();
        let snapshot = [("plugin.php", digest.as_str())];
        let mut files = Vec::new();
        assert!(super::collect_plugin_files(&dir, &dir, &mut files));
        files.sort();
        assert!(plugin_matches_hash_snapshot(&dir, &files, &snapshot));

        std::fs::write(&plugin, "<?php echo 'customized';").unwrap();
        assert!(!plugin_matches_hash_snapshot(&dir, &files, &snapshot));
        std::fs::write(&plugin, "<?php echo 'bundled';").unwrap();
        std::fs::write(dir.join("custom.php"), "<?php").unwrap();
        files.clear();
        assert!(super::collect_plugin_files(&dir, &dir, &mut files));
        files.sort();
        assert!(!plugin_matches_hash_snapshot(&dir, &files, &snapshot));

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn unique_path_keeps_existing_backup_intact() {
        let dir = std::env::temp_dir().join(format!("archivewiki-unique-path-{}", super::random_token()));
        std::fs::create_dir(&dir).unwrap();
        let original = dir.join("backup.zip");
        std::fs::write(&original, b"existing backup").unwrap();

        let next = unique_path(original.clone());
        assert_eq!(next.file_name().unwrap().to_string_lossy(), "backup (1).zip");
        assert_eq!(std::fs::read(&original).unwrap(), b"existing backup");
        assert!(!next.exists());

        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn app_navigation_accepts_only_the_exact_loopback_origin() {
        let origin = "http://127.0.0.1:47631";
        assert_eq!(
            app_path(origin, "http://127.0.0.1:47631/lib/exe/mediamanager.php?ns=wiki"),
            Some("/lib/exe/mediamanager.php?ns=wiki")
        );
        assert_eq!(app_path(origin, "http://127.0.0.1:47631"), Some(""));
        assert!(app_path(origin, "http://127.0.0.1:47631.evil.test/path").is_none());
        assert!(app_path(origin, "http://127.0.0.1:47632/path").is_none());
        assert!(app_path(origin, "https://example.com/").is_none());
    }

    #[test]
    fn reads_expected_digest_for_named_release_asset() {
        let json = r#"{"assets":[{"name":"php.tgz","digest":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"}]}"#;
        assert_eq!(
            release_asset_sha256_from_json(json, "php.tgz").unwrap(),
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn rejects_release_assets_without_sha256_metadata() {
        let json = r#"{"assets":[{"name":"php.tgz","digest":null}]}"#;
        assert!(release_asset_sha256_from_json(json, "php.tgz").is_err());
    }

    #[test]
    fn wiki_names_are_trimmed_and_cannot_escape_profile_directories() {
        assert_eq!(validate_wiki_name("  Family Archive  ").unwrap(), "Family Archive");
        assert!(validate_wiki_name("").is_err());
        assert!(validate_wiki_name("../Outside").is_err());
        assert!(validate_wiki_name("Family/Archive").is_err());
        assert!(validate_wiki_name("x".repeat(65).as_str()).is_err());
    }

    #[test]
    fn wiki_profiles_use_distinct_dokuwiki_sessions_without_changing_the_default() {
        assert_eq!(profile_session_name("default").unwrap(), None);
        assert_ne!(
            profile_session_name("wiki-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
            profile_session_name("wiki-bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb").unwrap()
        );
    }

    #[test]
    fn renaming_a_wiki_updates_its_backup_folder_and_keeps_backups() {
        let support = std::env::temp_dir().join(format!("archivewiki-rename-{}", super::random_token()));
        let backups = support.join("backups");
        let profile_id = format!("wiki-{}", "a".repeat(32));
        let profile_root = support.join("wikis").join(&profile_id);
        std::fs::create_dir_all(profile_root.join("wiki")).unwrap();
        std::fs::write(profile_root.join("wiki").join("doku.php"), "<?php").unwrap();
        std::fs::write(profile_root.join("name"), "Research").unwrap();
        let old_backup_dir = backups.join(super::profile_folder_name_for("Research", &profile_id));
        std::fs::create_dir_all(&old_backup_dir).unwrap();
        std::fs::write(old_backup_dir.join("wiki-backup.zip"), b"keep this backup").unwrap();
        let new_backup_dir = backups.join(super::profile_folder_name_for("Field Notes", &profile_id));
        std::fs::create_dir_all(&new_backup_dir).unwrap();
        std::fs::write(new_backup_dir.join("wiki-backup.zip"), b"keep the other backup").unwrap();
        let other_id = format!("wiki-{}", "c".repeat(32));
        let other_root = support.join("wikis").join(&other_id);
        std::fs::create_dir_all(other_root.join("wiki")).unwrap();
        std::fs::write(other_root.join("wiki").join("doku.php"), "<?php").unwrap();
        std::fs::write(other_root.join("name"), "Planning").unwrap();

        assert!(rename_wiki_profile(&support, &profile_id, "planning", Some(&backups)).is_err());
        let renamed = rename_wiki_profile(&support, &profile_id, "Field Notes", Some(&backups)).unwrap();
        assert_eq!(renamed.name, "Field Notes");
        assert_eq!(std::fs::read_to_string(profile_root.join("name")).unwrap(), "Field Notes");
        assert!(!old_backup_dir.exists());
        assert_eq!(std::fs::read(new_backup_dir.join("wiki-backup.zip")).unwrap(), b"keep the other backup");
        assert_eq!(std::fs::read(new_backup_dir.join("wiki-backup (1).zip")).unwrap(), b"keep this backup");
        assert!(rename_wiki_profile(&support, &profile_id, "field notes", Some(&backups)).is_ok());

        std::fs::remove_dir_all(support).unwrap();
    }

    #[test]
    fn deleting_a_wiki_protects_the_default_and_active_profiles_and_keeps_backups() {
        let support = std::env::temp_dir().join(format!("archivewiki-delete-{}", super::random_token()));
        let profile_id = format!("wiki-{}", "b".repeat(32));
        let profile_root = support.join("wikis").join(&profile_id);
        std::fs::create_dir_all(profile_root.join("wiki")).unwrap();
        std::fs::write(profile_root.join("wiki").join("doku.php"), "<?php").unwrap();
        std::fs::write(profile_root.join("name"), "Temporary").unwrap();
        let backup_dir = support.join("backups").join("Temporary backup");
        std::fs::create_dir_all(&backup_dir).unwrap();
        std::fs::write(backup_dir.join("archive.zip"), b"keep this backup").unwrap();

        assert!(delete_wiki_profile(&support, "default", &profile_id).is_err());
        assert!(delete_wiki_profile(&support, &profile_id, &profile_id).is_err());
        assert!(delete_wiki_profile(&support, &profile_id, "default").is_ok());
        assert!(!profile_root.exists());
        assert_eq!(std::fs::read(backup_dir.join("archive.zip")).unwrap(), b"keep this backup");

        std::fs::remove_dir_all(support).unwrap();
    }

    #[test]
    fn recent_pages_keep_the_default_key_and_are_namespaced_for_other_wikis() {
        assert_eq!(recent_pages_storage_key("default"), "archivewiki.recent-pages");
        assert_eq!(
            recent_pages_storage_key("wiki-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
            "archivewiki.recent-pages.wiki-aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn startup_routes_new_wikis_to_setup_and_existing_wikis_to_the_login_page() {
        assert_eq!(initial_wiki_route(false, false), "/install.php");
        assert_eq!(initial_wiki_route(false, true), "/install.php");
        assert_eq!(initial_wiki_route(true, false), "/doku.php");
        assert_eq!(initial_wiki_route(true, true), "/doku.php?do=login");
    }

    #[test]
    fn startup_screen_actions_are_recognized_without_accepting_other_custom_urls() {
        assert_eq!(startup_action("archivewiki://startup/login", true), Some("login"));
        assert_eq!(startup_action("archivewiki://startup/create", true), Some("create"));
        assert_eq!(startup_action("archivewiki://startup/continue", true), Some("continue"));
        assert_eq!(startup_action("archivewiki://startup/unknown", true), None);
        assert_eq!(startup_action("https://example.com/", true), None);
        assert_eq!(startup_action("archivewiki://startup/quit", false), None);
        assert!(STARTUP_HTML.contains("Log in to an existing wiki"));
        assert!(STARTUP_HTML.contains("Create a new wiki"));
    }

    #[test]
    fn session_token_is_appended_to_wiki_routes_with_or_without_query_parameters() {
        assert_eq!(
            wiki_url_with_session_token("http://127.0.0.1:47631", "/doku.php", "secret"),
            "http://127.0.0.1:47631/doku.php?aw=secret"
        );
        assert_eq!(
            wiki_url_with_session_token("http://127.0.0.1:47631", "/doku.php?do=login", "secret"),
            "http://127.0.0.1:47631/doku.php?do=login&aw=secret"
        );
    }

    #[test]
    fn profile_catalog_preserves_the_legacy_wiki_and_migrates_its_settings() {
        let support = std::env::temp_dir().join(format!("archivewiki-profiles-{}", super::random_token()));
        std::fs::create_dir_all(&support).unwrap();
        std::fs::write(support.join("backup-auto-off"), "").unwrap();
        std::fs::write(support.join("last-auto-backup"), "12345").unwrap();
        let legacy_restore = support.join("restore-previous").join("data");
        std::fs::create_dir_all(&legacy_restore).unwrap();
        std::fs::write(legacy_restore.join("page.txt"), "preserve me").unwrap();

        let profiles_root = support.join("wikis");
        let custom_id = format!("wiki-{}", "a".repeat(32));
        let custom_root = profiles_root.join(&custom_id);
        std::fs::create_dir_all(custom_root.join("wiki")).unwrap();
        std::fs::write(custom_root.join("name"), "Research").unwrap();
        std::fs::write(custom_root.join("wiki").join("doku.php"), "<?php").unwrap();

        let profiles = wiki_profiles(&support).unwrap();
        let default = profiles.iter().find(|profile| profile.id == "default").unwrap();
        assert_eq!(default.wiki, support.join("wiki"));
        assert!(default.root.join("backup-auto-off").is_file());
        assert_eq!(std::fs::read_to_string(default.root.join("last-auto-backup")).unwrap(), "12345");
        assert_eq!(
            std::fs::read_to_string(default.root.join("restore-previous").join("data").join("page.txt")).unwrap(),
            "preserve me"
        );
        assert!(profiles.iter().any(|profile| profile.id == custom_id && profile.name == "Research"));

        std::fs::remove_file(default.root.join("backup-auto-off")).unwrap();
        wiki_profiles(&support).unwrap();
        assert!(!default.root.join("backup-auto-off").exists());
        std::fs::write(support.join("active-wiki"), &custom_id).unwrap();
        let active = active_wiki_profile(&support).unwrap();
        assert_eq!(active.id, custom_id);
        assert_eq!(active.wiki, custom_root.join("wiki"));

        std::fs::remove_dir_all(support).unwrap();
    }
}
