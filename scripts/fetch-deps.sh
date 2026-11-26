#!/usr/bin/env bash
# Downloads DokuWiki (stable) and a static PHP CLI for macOS arm64 into ./resources
set -euo pipefail
cd "$(dirname "$0")/.."
RES=resources; mkdir -p "$RES" build

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
    echo "    vendor/dokuwiki.tar not found; downloading stable"
    curl -fL "https://download.dokuwiki.org/src/dokuwiki/dokuwiki-stable.tgz" -o build/dokuwiki.tgz
    SRC=build/dokuwiki.tgz
  fi
  rm -rf "$RES/dokuwiki"; mkdir -p "$RES/dokuwiki"
  tar -xf "$SRC" -C "$RES/dokuwiki" --strip-components=1 --no-same-owner
  echo "    DokuWiki $(cat "$RES/dokuwiki/VERSION")"
fi

if [ ! -x "$RES/php" ]; then
  echo "==> static PHP"
  export PHP_MINOR="${PHP_MINOR:-8.3}"
  DEFAULT_REPO="nemcmxciii/archivewiki"
  REPO="${ARCHIVEWIKI_REPO:-${GITHUB_REPOSITORY:-$DEFAULT_REPO}}"
  if [ -z "${PHP_URL:-}" ]; then
    echo "    looking for a PHP build in github.com/$REPO releases (run the 'Build PHP' workflow to create one; the repo must be public)"
    export REPO
    PHP_URL=$(curl -fsSL "https://github.com/$REPO/releases.atom" 2>/dev/null | python3 -c '
import sys,re,os
minor=re.escape(os.environ["PHP_MINOR"]); repo=os.environ["REPO"]
txt=sys.stdin.read()
vs=[tuple(map(int,v.split("."))) for v in re.findall(r"/releases/tag/php-(%s\.\d+)" % minor, txt)]
if vs:
    v=".".join(map(str,max(vs)))
    print("https://github.com/%s/releases/download/php-%s/php-%s-cli-macos-aarch64.tar.gz" % (repo,v,v))
else:
    print("")' || true)
  fi
  if [ -z "${PHP_URL:-}" ]; then
    echo "    none found; falling back to static-php.dev 'bulk' (NO pdo_sqlite: the build will stop at the check below)"
    BASE="https://dl.static-php.dev/static-php-cli/bulk"
    NAME=$(curl -fsSL "$BASE/?format=json" | python3 -c '
import json,sys,re,os
minor=os.environ["PHP_MINOR"]
pat=re.compile(r"^php-(%s\.\d+)-cli-macos-aarch64\.tar\.gz$" % re.escape(minor))
c=[]
for i in json.load(sys.stdin):
    m=pat.match(i["name"])
    if m: c.append((tuple(map(int,m.group(1).split("."))), i["name"]))
print(max(c)[1])')
    PHP_URL="$BASE/$NAME"
  fi
  echo "    $PHP_URL"
  curl -fL "$PHP_URL" -o build/php.tgz
  if [ -n "${PHP_SHA256:-}" ]; then
    if command -v sha256sum >/dev/null 2>&1; then
      echo "$PHP_SHA256  build/php.tgz" | sha256sum -c - >/dev/null
    else
      echo "$PHP_SHA256  build/php.tgz" | shasum -a 256 -c - >/dev/null
    fi
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
