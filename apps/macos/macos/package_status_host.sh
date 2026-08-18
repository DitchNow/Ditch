#!/bin/sh
set -eu

HELPER_APP="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Library/LoginItems/The Ditch Runtime.app"
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
DITCHD_LIBRARY="$WORKSPACE_ROOT/target/$CARGO_PROFILE/libditchd.a"
DITCH_CLI_SOURCE="$WORKSPACE_ROOT/target/$CARGO_PROFILE/ditch_cli"
test -x "$DITCHD_SOURCE"
test -f "$DITCHD_LIBRARY"
test -x "$DITCH_CLI_SOURCE"

mkdir -p "$HELPER_MACOS"

# Xcode's user-script sandbox cannot write Swift's default module cache under
# ~/.cache. Keep all compiler intermediates inside this build.
SWIFT_CACHE="${TARGET_TEMP_DIR:-${TMPDIR:-/tmp}/the-ditch-swift-module-cache}"
mkdir -p "$SWIFT_CACHE"
export SWIFT_MODULECACHE_PATH="$SWIFT_CACHE"
export CLANG_MODULE_CACHE_PATH="$SWIFT_CACHE"

/usr/bin/swiftc -parse-as-library "$PROJECT_DIR/StatusHost/StatusHost.swift" \
  "$DITCHD_LIBRARY" \
  -framework Cocoa \
  -o "$HELPER_MACOS/ditchd"

cat > "$HELPER_CONTENTS/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleExecutable</key>
  <string>ditchd</string>
  <key>CFBundleIdentifier</key>
  <string>ai.theditch.runtime</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>The Ditch Runtime</string>
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

cp "$DITCH_CLI_SOURCE" "$HELPER_MACOS/ditch_cli"
cp "$DITCH_CLI_SOURCE" "$MAIN_MACOS/ditch_cli"
cp "$HELPER_MACOS/ditchd" "$MAIN_MACOS/ditchd"

# Verify the copies before signing (codesign changes Mach-O bytes). `cmp` is
# killed by Xcode's script sandbox for Mach-O inputs, so verify executability
# and exact byte size here; `cp` itself already exits nonzero on write failure.
test -x "$MAIN_MACOS/ditchd"
test -x "$MAIN_MACOS/ditch_cli"
test "$(stat -f %z "$HELPER_MACOS/ditchd")" = "$(stat -f %z "$MAIN_MACOS/ditchd")"
test "$(stat -f %z "$DITCH_CLI_SOURCE")" = "$(stat -f %z "$MAIN_MACOS/ditch_cli")"

# Rust and Swift linkers already emit ad-hoc-signed Mach-O executables. Avoid
# asking macOS codesign to replace those signatures repeatedly: clean builds
# can otherwise fail with an intermittent "internal error in Code Signing
# subsystem". Seal the nested runtime bundle here; Xcode signs and seals the
# containing application afterward.
/usr/bin/codesign --force --sign - "$HELPER_APP"

rm -rf "$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Library/LoginItems/The Ditch Status.app"
rm -f "$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/MacOS/ditch-status-host"
