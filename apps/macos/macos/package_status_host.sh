#!/bin/sh
set -eu

HELPER_APP="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Library/LoginItems/The Ditch Status.app"
HELPER_CONTENTS="$HELPER_APP/Contents"
HELPER_MACOS="$HELPER_CONTENTS/MacOS"
MAIN_MACOS="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/MacOS"

mkdir -p "$HELPER_MACOS"

/usr/bin/swiftc -parse-as-library "$PROJECT_DIR/StatusHost/StatusHost.swift" \
  -framework Cocoa \
  -o "$HELPER_MACOS/The Ditch Status"

cat > "$HELPER_CONTENTS/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>The Ditch Status</string>
  <key>CFBundleIdentifier</key>
  <string>ai.theditch.status</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>The Ditch Status</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>1.0.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSMinimumSystemVersion</key>
  <string>10.15</string>
  <key>LSUIElement</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
</dict>
</plist>
PLIST

if [ -f "$PROJECT_DIR/../../../target/debug/ditchd" ]; then
  cp "$PROJECT_DIR/../../../target/debug/ditchd" "$HELPER_MACOS/ditchd"
  cp "$PROJECT_DIR/../../../target/debug/ditchd" "$MAIN_MACOS/ditchd"
fi

if [ -f "$PROJECT_DIR/../../../target/debug/ditch_cli" ]; then
  cp "$PROJECT_DIR/../../../target/debug/ditch_cli" "$HELPER_MACOS/ditch_cli"
  cp "$PROJECT_DIR/../../../target/debug/ditch_cli" "$MAIN_MACOS/ditch_cli"
fi

/usr/bin/codesign --force --sign - "$HELPER_MACOS/The Ditch Status"
if [ -f "$HELPER_MACOS/ditchd" ]; then
  /usr/bin/codesign --force --sign - "$HELPER_MACOS/ditchd"
fi
if [ -f "$HELPER_MACOS/ditch_cli" ]; then
  /usr/bin/codesign --force --sign - "$HELPER_MACOS/ditch_cli"
fi
if [ -f "$MAIN_MACOS/ditchd" ]; then
  /usr/bin/codesign --force --sign - "$MAIN_MACOS/ditchd"
fi
if [ -f "$MAIN_MACOS/ditch_cli" ]; then
  /usr/bin/codesign --force --sign - "$MAIN_MACOS/ditch_cli"
fi
/usr/bin/codesign --force --sign - "$HELPER_APP"

rm -f "$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/MacOS/ditch-status-host"
