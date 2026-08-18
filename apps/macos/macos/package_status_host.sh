#!/bin/sh
set -eu

HELPER_APP="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Library/LoginItems/The Ditch Status.app"
HELPER_CONTENTS="$HELPER_APP/Contents"
HELPER_MACOS="$HELPER_CONTENTS/MacOS"
MAIN_MACOS="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/MacOS"
WORKSPACE_ROOT="$PROJECT_DIR/../../.."

case "${CONFIGURATION:-Debug}" in
  Release|Profile)
    CARGO_PROFILE="release"
    CARGO_FLAGS="--release"
    ;;
  *)
    CARGO_PROFILE="debug"
    CARGO_FLAGS=""
    ;;
esac

cd "$WORKSPACE_ROOT"
# The app must never package whatever happens to be left in target/. Build the
# runtime used by this exact app build first, and fail if Cargo cannot produce it.
/usr/bin/env cargo build --locked -p ditchd -p ditch_cli $CARGO_FLAGS
DITCHD_SOURCE="$WORKSPACE_ROOT/target/$CARGO_PROFILE/ditchd"
DITCH_CLI_SOURCE="$WORKSPACE_ROOT/target/$CARGO_PROFILE/ditch_cli"
test -x "$DITCHD_SOURCE"
test -x "$DITCH_CLI_SOURCE"

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

cp "$DITCHD_SOURCE" "$HELPER_MACOS/ditchd"
cp "$DITCHD_SOURCE" "$MAIN_MACOS/ditchd"
cp "$DITCH_CLI_SOURCE" "$HELPER_MACOS/ditch_cli"
cp "$DITCH_CLI_SOURCE" "$MAIN_MACOS/ditch_cli"

# Verify the copies before signing (codesign changes Mach-O bytes). `cmp` is
# killed by Xcode's script sandbox for Mach-O inputs, so verify executability
# and exact byte size here; `cp` itself already exits nonzero on write failure.
test -x "$MAIN_MACOS/ditchd"
test -x "$MAIN_MACOS/ditch_cli"
test "$(stat -f %z "$DITCHD_SOURCE")" = "$(stat -f %z "$MAIN_MACOS/ditchd")"
test "$(stat -f %z "$DITCH_CLI_SOURCE")" = "$(stat -f %z "$MAIN_MACOS/ditch_cli")"

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
