#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PHP="$ROOT/resources/php"
if [[ ! -x "$PHP" ]]; then
  PHP="$(command -v php)"
fi

TEMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/archivewiki-router.XXXXXX")"
SERVER_PID=""
cleanup() {
  if [[ -n "$SERVER_PID" ]]; then
    kill "$SERVER_PID" 2>/dev/null || true
    wait "$SERVER_PID" 2>/dev/null || true
  fi
  rm -rf "$TEMP_DIR"
}
trap cleanup EXIT

mkdir -p "$TEMP_DIR/wiki/lib/exe" "$TEMP_DIR/wiki/lib/tpl/dokuwiki"
printf '<?php echo "reset-route-ok";\n' > "$TEMP_DIR/wiki/doku.php"
printf '<?php echo "css-route-ok";\n' > "$TEMP_DIR/wiki/lib/exe/css.php"
printf '<?php echo "media-route-ok";\n' > "$TEMP_DIR/wiki/lib/exe/fetch.php"
printf '<?php echo "ajax-route";\n' > "$TEMP_DIR/wiki/lib/exe/ajax.php"
printf 'body { color: black; }\n' > "$TEMP_DIR/wiki/lib/tpl/dokuwiki/style.css"

PORT="$(python3 -c 'import socket; s = socket.socket(); s.bind(("127.0.0.1", 0)); print(s.getsockname()[1]); s.close()')"
BASE_URL="http://127.0.0.1:$PORT"
RESET_TOKEN="0123456789abcdef0123456789abcdef"
LAUNCH_TOKEN="test-launch-secret"
ARCHIVEWIKI_TOKEN="$LAUNCH_TOKEN" PHP_CLI_SERVER_WORKERS=1 "$PHP" -n -S "127.0.0.1:$PORT" \
  -t "$TEMP_DIR/wiki" "$ROOT/src/router.php" > "$TEMP_DIR/server.log" 2>&1 &
SERVER_PID=$!

for _ in $(seq 1 50); do
  if curl --silent --output /dev/null "$BASE_URL/doku.php"; then
    break
  fi
  sleep 0.1
done

expect_status() {
  local expected="$1"
  shift
  local actual
  actual="$(curl --silent --show-error --output "$TEMP_DIR/response" --write-out '%{http_code}' "$@")"
  if [[ "$actual" != "$expected" ]]; then
    printf 'Expected HTTP %s, got %s for %s\n' "$expected" "$actual" "$*" >&2
    cat "$TEMP_DIR/response" >&2
    exit 1
  fi
}

RESET_URL="$BASE_URL/doku.php?do=resendpwd&pwauth=$RESET_TOKEN"
expect_status 200 "$RESET_URL"
grep -q 'reset-route-ok' "$TEMP_DIR/response"
expect_status 403 "$BASE_URL/doku.php"
expect_status 403 "$BASE_URL/doku.php?do=login&pwauth=$RESET_TOKEN"
expect_status 403 "$BASE_URL/doku.php?do=resendpwd&pwauth=invalid"
expect_status 200 -H "Referer: $RESET_URL" "$BASE_URL/lib/exe/css.php"
expect_status 200 -H "Referer: $RESET_URL" "$BASE_URL/lib/exe/fetch.php?media=wiki%3Alogo.png"
expect_status 403 -H "Referer: $RESET_URL" "$BASE_URL/lib/exe/fetch.php?media=private%3Apage.png"
expect_status 200 -H "Referer: $RESET_URL" "$BASE_URL/lib/tpl/dokuwiki/style.css"
expect_status 403 "$BASE_URL/lib/exe/css.php"
expect_status 403 -H "Referer: http://example.com/doku.php?do=resendpwd&pwauth=$RESET_TOKEN" "$BASE_URL/lib/exe/css.php"
expect_status 403 -H "Referer: $RESET_URL" "$BASE_URL/lib/exe/ajax.php"
expect_status 200 --request POST --data "do=resendpwd&token=$RESET_TOKEN" "$BASE_URL/doku.php"
expect_status 403 -H 'Host: example.com' "$RESET_URL"
expect_status 302 --cookie-jar "$TEMP_DIR/cookies" "$BASE_URL/doku.php?aw=$LAUNCH_TOKEN"
expect_status 200 --cookie "$TEMP_DIR/cookies" "$BASE_URL/doku.php"

echo "Password-reset router checks passed."
