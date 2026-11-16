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
  else
    echo "    vendor/dokuwiki.tar not found; downloading stable"
    curl -fL "https://download.dokuwiki.org/src/dokuwiki/dokuwiki-stable.tgz" -o build/dokuwiki.tgz
    SRC=build/dokuwiki.tgz
  fi
  rm -rf "$RES/dokuwiki"; mkdir -p "$RES/dokuwiki"
  tar -xf "$SRC" -C "$RES/dokuwiki" --strip-components=1 --no-same-owner
  rm -f "$RES/dokuwiki/install.php"
  echo "    DokuWiki $(cat "$RES/dokuwiki/VERSION")"
fi

if [ ! -x "$RES/php" ]; then
  echo "==> static PHP"
  FLAVOR="${FLAVOR:-bulk}"            # common | bulk (bulk has more extensions)
  export PHP_MINOR="${PHP_MINOR:-8.3}"
  BASE="https://dl.static-php.dev/static-php-cli/$FLAVOR"
  if [ -z "${PHP_URL:-}" ]; then
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
  tar -xzf build/php.tgz -C build php
  mv build/php "$RES/php"; chmod +x "$RES/php"
  xattr -d com.apple.quarantine "$RES/php" 2>/dev/null || true
fi

echo "==> sanity check"
"$RES/php" -v | head -1
for m in xml dom mbstring zlib gd session; do
  "$RES/php" -m | grep -qi "^$m$" && echo "$m ok" || echo "note: $m missing"
done
