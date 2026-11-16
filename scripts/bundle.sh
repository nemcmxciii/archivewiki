#!/usr/bin/env bash
# Builds dist/ArchiveWiki.app (+ .dmg)
set -euo pipefail
cd "$(dirname "$0")/.."
./scripts/fetch-deps.sh
cargo build --release

APP="dist/ArchiveWiki.app"
rm -rf "$APP"; mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
cp target/release/archivewiki "$APP/Contents/MacOS/"
cp resources/php "$APP/Contents/Resources/php"
cp -R resources/dokuwiki "$APP/Contents/Resources/dokuwiki"

ICON_KEY=""
if [ -f assets/icon.png ]; then   # optional: 1024x1024 PNG
  S=build/AppIcon.iconset; rm -rf "$S"; mkdir -p "$S"
  for sz in 16 32 128 256 512; do
    sips -z $sz $sz assets/icon.png --out "$S/icon_${sz}x${sz}.png" >/dev/null
    sips -z $((sz*2)) $((sz*2)) assets/icon.png --out "$S/icon_${sz}x${sz}@2x.png" >/dev/null
  done
  iconutil -c icns "$S" -o "$APP/Contents/Resources/AppIcon.icns"
  ICON_KEY="<key>CFBundleIconFile</key><string>AppIcon</string>"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
  <key>CFBundleName</key><string>ArchiveWiki</string>
  <key>CFBundleDisplayName</key><string>ArchiveWiki</string>
  <key>CFBundleIdentifier</key><string>app.archivewiki.desktop</string>
  <key>CFBundleExecutable</key><string>archivewiki</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>0.1.0</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>12.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>LSApplicationCategoryType</key><string>public.app-category.productivity</string>
  $ICON_KEY
</dict></plist>
PLIST

codesign --force --deep --sign - "$APP"
hdiutil create -volname "ArchiveWiki" -srcfolder "$APP" -ov -format UDZO "dist/ArchiveWiki.dmg" >/dev/null
echo "Built: $APP"
echo "       dist/ArchiveWiki.dmg"
