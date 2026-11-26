#!/usr/bin/env bash
# Downloads DokuWiki (stable) and a static PHP CLI for macOS arm64 into ./resources
set -euo pipefail
cd "$(dirname "$0")/.."
RES=resources; mkdir -p "$RES" build
. ./dependencies.lock

if [ ! -f "$RES/dokuwiki/doku.php" ]; then
  echo "==> DokuWiki"
  if [ -f vendor/dokuwiki.tar ]; then
    echo "    using vendor/dokuwiki.tar"
    SRC=vendor/dokuwiki.tar
    if [ -f dependencies.sha256 ]; then
      (grep "  vendor/dokuwiki.tar$" dependencies.sha256 | sha256sum -c - >/dev/null 2>&1) || \
      (grep "  vendor/dokuwiki.tar$" dependencies.sha256 | shasum -a 256 -c - >/dev/null 2>&1) || {
        echo "ERROR: vendor/dokuwiki.tar failed SHA-256 verification"
        exit 1
      }
    fi
  else
    echo "    vendor/dokuwiki.tar not found; fetching pinned release"
    VERSION="${DOKUWIKI_VERSION:-$LOCKED_DOKUWIKI_VERSION}"
    EXPECTED="${DOKUWIKI_SHA256:-$LOCKED_DOKUWIKI_SHA256}"
    [[ "$VERSION" =~ ^[0-9]{4}-[0-9]{2}-[0-9]{2}[a-z]?$ ]] || {
      echo "ERROR: could not resolve a valid DokuWiki release version"
      exit 1
    }
    TAG="release-$VERSION"
    NAME="dokuwiki-$VERSION.tgz"
    curl -fL "https://github.com/dokuwiki/dokuwiki/releases/download/$TAG/$NAME" -o build/dokuwiki.tgz
    if command -v sha256sum >/dev/null 2>&1; then
      echo "$EXPECTED  build/dokuwiki.tgz" | sha256sum -c - >/dev/null
    else
      echo "$EXPECTED  build/dokuwiki.tgz" | shasum -a 256 -c - >/dev/null
    fi
    SRC=build/dokuwiki.tgz
  fi
  rm -rf "$RES/dokuwiki"; mkdir -p "$RES/dokuwiki"
  tar -xf "$SRC" -C "$RES/dokuwiki" --strip-components=1 --no-same-owner
  echo "    DokuWiki $(cat "$RES/dokuwiki/VERSION")"
fi

if [ ! -x "$RES/php" ]; then
  echo "==> static PHP"
  DEFAULT_REPO="nemcmxciii/archivewiki"
  REPO="${ARCHIVEWIKI_REPO:-${GITHUB_REPOSITORY:-$DEFAULT_REPO}}"
  PHP_VERSION="${PHP_VERSION:-$LOCKED_PHP_VERSION}"
  if [ -z "${PHP_URL:-}" ]; then
    PHP_URL="https://github.com/$REPO/releases/download/php-$PHP_VERSION/php-$PHP_VERSION-cli-macos-aarch64.tar.gz"
  fi
  echo "    $PHP_URL"
  export PHP_URL REPO
  if [ -z "${PHP_SHA256:-}" ]; then
    if [ "$PHP_URL" = "https://github.com/$DEFAULT_REPO/releases/download/php-$LOCKED_PHP_VERSION/php-$LOCKED_PHP_VERSION-cli-macos-aarch64.tar.gz" ] && [ "$REPO" = "$DEFAULT_REPO" ]; then
      PHP_SHA256="$LOCKED_PHP_SHA256"
    else
      PHP_VERSION=$(python3 -c '
import os,re
m=re.search(r"/php-(\d+\.\d+\.\d+)/php-\d+\.\d+\.\d+-cli-macos-aarch64\.tar\.gz$", os.environ["PHP_URL"])
if not m: raise SystemExit("Set PHP_SHA256 when PHP_URL does not identify a project release")
print(m.group(1))')
      PHP_SHA256=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/tags/php-$PHP_VERSION" | python3 -c '
import json,sys
name="php-%s-cli-macos-aarch64.tar.gz" % sys.argv[1]
release=json.load(sys.stdin)
asset=next((a for a in release.get("assets",[]) if a.get("name")==name),None)
digest=asset.get("digest","") if asset else ""
if not digest.startswith("sha256:") or len(digest) != 71:
    raise SystemExit("GitHub release does not provide a valid SHA-256 digest")
print(digest[7:])' "$PHP_VERSION")
    fi
  fi
  curl -fL "$PHP_URL" -o build/php.tgz
  if command -v sha256sum >/dev/null 2>&1; then
    echo "$PHP_SHA256  build/php.tgz" | sha256sum -c - >/dev/null
  else
    echo "$PHP_SHA256  build/php.tgz" | shasum -a 256 -c - >/dev/null
  fi
  tar -xzf build/php.tgz -C build php
  mv build/php "$RES/php"; chmod +x "$RES/php"
  xattr -d com.apple.quarantine "$RES/php" 2>/dev/null || true
fi

echo "==> sanity check"
"$RES/php" -v | head -1
FAIL=0
MODS=$("$RES/php" -m)   # capture once: piping `php -m` into `grep -q` can die of SIGPIPE under pipefail and mis-report
has() { grep -qi "^$1\$" <<<"$MODS"; }
for m in xml dom mbstring zlib session pdo_sqlite sqlite3 ctype; do
  if has "$m"; then echo "$m ok"; else echo "REQUIRED extension missing: $m"; FAIL=1; fi
done
for m in gd curl openssl intl; do
  if has "$m"; then echo "$m ok"; else echo "note: $m missing (optional)"; fi
done
if [ "$FAIL" = "1" ]; then
  echo
  echo "This PHP lacks required extensions. Run the 'Build PHP' workflow on GitHub, then try again."
  rm -f "$RES/php"
  exit 1
fi
