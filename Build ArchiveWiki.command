#!/usr/bin/env bash
# Double-click me. Builds ArchiveWiki.app + ArchiveWiki.dmg on this Mac (Apple silicon).
cd "$(dirname "$0")" || exit 1
pause() { echo; read -n1 -r -p "Press any key to close…"; echo; }
fail()  { echo; echo "✗ $1"; pause; exit 1; }

echo "== ArchiveWiki builder =="
[ "$(uname -m)" = "arm64" ] || fail "This build targets Apple silicon (arm64)."

# 1. Xcode command line tools
if ! xcode-select -p >/dev/null 2>&1; then
  echo "Apple's command line tools are needed. A system installer will open."
  xcode-select --install
  fail "Finish that installer, then double-click this file again."
fi

# 2. Rust
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"
if ! command -v cargo >/dev/null 2>&1; then
  echo "Rust is not installed. It is needed only to build the app (installs to ~/.cargo, no admin rights)."
  read -r -p "Install Rust now from rustup.rs? [y/N] " a
  [ "$a" = "y" ] || [ "$a" = "Y" ] || fail "Cancelled."
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal || fail "Rust install failed."
  . "$HOME/.cargo/env"
fi

# 3. Build
bash scripts/bundle.sh || fail "Build failed. Copy the messages above into Claude and it will fix them."

echo
echo "✓ Built:  dist/ArchiveWiki.app   and   dist/ArchiveWiki.dmg"
read -r -p "Copy ArchiveWiki.app to /Applications now? [y/N] " a
if [ "$a" = "y" ] || [ "$a" = "Y" ]; then
  rm -rf "/Applications/ArchiveWiki.app"
  ditto "dist/ArchiveWiki.app" "/Applications/ArchiveWiki.app" && open "/Applications/ArchiveWiki.app"
else
  open dist
fi
pause
