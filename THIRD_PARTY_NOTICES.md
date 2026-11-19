# Third-party software bundled with ArchiveWiki

ArchiveWiki is a native shell around other people's open-source work. Please keep this notice with any copy you pass on.

| Component | What it is | License | Source |
|---|---|---|---|
| DokuWiki | The wiki engine | GPL-2.0 | https://www.dokuwiki.org |
| PHP (static build) | Runs DokuWiki; built with static-php-cli | PHP License v3.01 (plus the licenses of statically linked libraries) | https://www.php.net, https://github.com/crazywhalecc/static-php-cli |
| struct | Structured data for pages | GPL-2.0 | https://github.com/cosmocode/dokuwiki-plugin-struct |
| sqlite | SQLite helper (needed by struct) | GPL-2.0 (listed on its DokuWiki extension page; the repository ships no license file) | https://github.com/cosmocode/sqlite |
| bureaucracy | Forms | GPL-2.0 | https://github.com/splitbrain/dokuwiki-plugin-bureaucracy |
| move | Move and rename pages with link updates | GPL-2.0 | https://github.com/michitux/dokuwiki-plugin-move |
| tag | Tagging | GPL-2.0 | https://github.com/dokufreaks/plugin-tag |
| pagelist | Page lists (used by tag) | GPL-2.0 | https://github.com/dokufreaks/plugin-pagelist |
| include | Include pages in pages | GPL-2.0 | https://github.com/dokufreaks/plugin-include |
| wrap | Layout and style blocks | GPL-2.0 | https://github.com/selfthinker/dokuwiki_plugin_wrap |
| gallery | Image galleries | GPL-2.0 | https://github.com/splitbrain/dokuwiki-plugin-gallery |

Each plugin's own license file (LICENSE or COPYING) is kept inside its folder under `vendor/plugins/`.
The plugin versions are the repositories' default branches as downloaded on 2026-11-17; their dates are in each `plugin.info.txt`.

The GPL components mean that if you share a built copy of ArchiveWiki with anyone else, you should also make the corresponding source
available (this repository does that) and keep these notices. For purely personal use on your own Mac nothing more is required.
