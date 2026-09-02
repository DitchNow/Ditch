#!/bin/sh
set -eu

HELPER_APP="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Library/LoginItems/The Ditch Runtime.app"
HELPER_CONTENTS="$HELPER_APP/Contents"
HELPER_MACOS="$HELPER_CONTENTS/MacOS"
MAIN_MACOS="$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/MacOS"
WORKSPACE_ROOT="$PROJECT_DIR/../../.."
DEPLOYMENT_TARGET="${MACOSX_DEPLOYMENT_TARGET:-11.0}"
BUILD_ARCH="${CURRENT_ARCH:-}"
case "$BUILD_ARCH" in
  ""|undefined_arch) BUILD_ARCH="$(uname -m)" ;;
esac

# These values are public build configuration, not credentials. Direct Flutter
# and Xcode builds retain the historical production default. The
# scripts/macos-app wrapper sets all three explicitly for staging/production.
DITCH_DEPLOYMENT_ENVIRONMENT="${DITCH_DEPLOYMENT_ENVIRONMENT:-production}"
DITCH_EDITION="${DITCH_EDITION:-commercial}"
DITCH_RELAY_ORIGIN="${DITCH_RELAY_ORIGIN:-https://relay.ditchnow.nl}"
DITCH_UPDATE_ALLOWED_HOSTS="${DITCH_UPDATE_ALLOWED_HOSTS:-relay.ditchnow.nl,downloads.ditchnow.nl,updates.ditchnow.nl}"

case "$DITCH_DEPLOYMENT_ENVIRONMENT" in
  staging|production) ;;
  *) echo "error: DITCH_DEPLOYMENT_ENVIRONMENT must be staging or production" >&2; exit 1 ;;
esac
case "$DITCH_RELAY_ORIGIN" in
  https://*) ;;
  *) echo "error: DITCH_RELAY_ORIGIN must be an HTTPS origin" >&2; exit 1 ;;
esac
DITCH_RELAY_HOST=${DITCH_RELAY_ORIGIN#https://}
case "$DITCH_RELAY_HOST" in
  ""|*/*|*'?'*|*'#'*|*'@'*|*:*)
    echo "error: DITCH_RELAY_ORIGIN must contain only an HTTPS scheme and hostname" >&2
    exit 1
    ;;
esac
case "$DITCH_UPDATE_ALLOWED_HOSTS" in
  *[!A-Za-z0-9.,-]*)
    echo "error: DITCH_UPDATE_ALLOWED_HOSTS contains an invalid character" >&2
    exit 1
    ;;
esac
case ",$DITCH_UPDATE_ALLOWED_HOSTS," in
  *,"$DITCH_RELAY_HOST",*) ;;
  *) echo "error: DITCH_UPDATE_ALLOWED_HOSTS must include $DITCH_RELAY_HOST" >&2; exit 1 ;;
esac

PRODUCTION_RELAY_ORIGIN=https://relay.ditchnow.nl
if [ "$DITCH_DEPLOYMENT_ENVIRONMENT" = production ] && [ "$DITCH_RELAY_ORIGIN" != "$PRODUCTION_RELAY_ORIGIN" ]; then
  echo "error: production must use $PRODUCTION_RELAY_ORIGIN" >&2
  exit 1
fi
if [ "$DITCH_DEPLOYMENT_ENVIRONMENT" = staging ] && [ "$DITCH_RELAY_ORIGIN" = "$PRODUCTION_RELAY_ORIGIN" ]; then
  echo "error: staging must not use the production Relay" >&2
  exit 1
fi
export DITCH_DEPLOYMENT_ENVIRONMENT DITCH_EDITION DITCH_RELAY_ORIGIN DITCH_UPDATE_ALLOWED_HOSTS

# Keep every object linked into the nested helper on the same explicit minimum
# OS version. Without this, a newer Xcode stamps its own host OS as the Swift
# executable's minimum even when the enclosing Flutter app supports older Macs.
MACOSX_DEPLOYMENT_TARGET="$DEPLOYMENT_TARGET"
export MACOSX_DEPLOYMENT_TARGET

# Archive builds run outside the user's interactive shell, so Homebrew and
# rustup are usually absent from PATH. Resolve Cargo explicitly instead of
# depending on whichever environment happened to launch Xcode.
CARGO_BIN=""
for CANDIDATE in \
  "${CARGO:-}" \
  "/opt/homebrew/bin/cargo" \
  "/usr/local/bin/cargo" \
  "$HOME/.cargo/bin/cargo"
do
  if [ -n "$CANDIDATE" ] && [ -x "$CANDIDATE" ]; then
    CARGO_BIN="$CANDIDATE"
    break
  fi
done
if [ -z "$CARGO_BIN" ]; then
  echo "error: Cargo was not found. Install Rust or set CARGO to the cargo executable." >&2
  exit 1
fi

# Cargo launches rustc and linker helpers by name. Make the directory that
# supplied Cargo visible to those child processes in Xcode's restricted
# Archive environment as well.
RUST_TOOLCHAIN_BIN="$(dirname "$CARGO_BIN")"
PATH="$RUST_TOOLCHAIN_BIN:$PATH"
export PATH

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
DITCH_BUILD_IDENTIFIER="${DITCH_BUILD_IDENTIFIER:-$("$WORKSPACE_ROOT/scripts/build-identifier")}"
case "$DITCH_BUILD_IDENTIFIER" in
  *[!A-Za-z0-9._-]*) echo 'error: invalid DITCH_BUILD_IDENTIFIER' >&2; exit 1 ;;
esac
DITCH_BUILD_NUMBER="${DITCH_BUILD_NUMBER:-${FLUTTER_BUILD_NUMBER:-0}}"
DITCH_APP_VERSION="${DITCH_APP_VERSION:-${FLUTTER_BUILD_NAME:-0.1.0}}"
DITCH_RELEASE_SEQUENCE="${DITCH_RELEASE_SEQUENCE:-$DITCH_BUILD_NUMBER}"
DITCH_COMMUNITY_REVISION="${DITCH_COMMUNITY_REVISION:-$(tr -d '\r\n' < "$WORKSPACE_ROOT/COMMUNITY_REVISION")}"
export DITCH_APP_VERSION DITCH_BUILD_IDENTIFIER DITCH_BUILD_NUMBER DITCH_RELEASE_SEQUENCE DITCH_COMMUNITY_REVISION
# The app must never package whatever happens to be left in target/. Build the
# runtime used by this exact app build first, and fail if Cargo cannot produce it.
"$CARGO_BIN" build --locked -p ditchd -p ditch_cli $CARGO_FLAGS
DITCHD_SOURCE="$WORKSPACE_ROOT/target/$CARGO_PROFILE/ditchd"
DITCHD_LIBRARY="$WORKSPACE_ROOT/target/$CARGO_PROFILE/libditchd.a"
DITCH_CLI_SOURCE="$WORKSPACE_ROOT/target/$CARGO_PROFILE/ditch_cli"
test -x "$DITCHD_SOURCE"
test -f "$DITCHD_LIBRARY"
test -x "$DITCH_CLI_SOURCE"
case "$BUILD_ARCH" in
  arm64|aarch64) HOST_REMOTE_TARGET="aarch64-apple-darwin" ;;
  x86_64) HOST_REMOTE_TARGET="x86_64-apple-darwin" ;;
  *) echo "error: unsupported remote runtime architecture $BUILD_ARCH" >&2; exit 1 ;;
esac

mkdir -p "$HELPER_MACOS"
HELPER_RESOURCES="$HELPER_CONTENTS/Resources/RemoteRuntimes"
MAIN_RESOURCES="$TARGET_BUILD_DIR/$UNLOCALIZED_RESOURCES_FOLDER_PATH"
mkdir -p "$MAIN_RESOURCES"
rm -f "$MAIN_MACOS/ditchd"

# AppKit independently validates the short-lived Commercial update feed. Give
# it the same build environment and Relay host that were compiled into Rust.
cat > "$MAIN_RESOURCES/DitchEnvironment.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN"
  "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>DeploymentEnvironment</key>
  <string>$DITCH_DEPLOYMENT_ENVIRONMENT</string>
  <key>Edition</key>
  <string>$DITCH_EDITION</string>
  <key>RelayOrigin</key>
  <string>$DITCH_RELAY_ORIGIN</string>
  <key>AllowedUpdateHosts</key>
  <string>$DITCH_UPDATE_ALLOWED_HOSTS</string>
  <key>BuildIdentifier</key>
  <string>$DITCH_BUILD_IDENTIFIER</string>
  <key>BuildNumber</key>
  <string>$DITCH_BUILD_NUMBER</string>
  <key>ReleaseSequence</key>
  <integer>$DITCH_RELEASE_SEQUENCE</integer>
  <key>CommunityRevision</key>
  <string>$DITCH_COMMUNITY_REVISION</string>
</dict>
</plist>
PLIST
/usr/bin/plutil -lint "$MAIN_RESOURCES/DitchEnvironment.plist" >/dev/null

# Xcode's user-script sandbox cannot write Swift's default module cache under
# ~/.cache. Keep all compiler intermediates inside this build.
SWIFT_CACHE="${TARGET_TEMP_DIR:-${TMPDIR:-/tmp}/the-ditch-swift-module-cache}"
mkdir -p "$SWIFT_CACHE"
export SWIFT_MODULECACHE_PATH="$SWIFT_CACHE"
export CLANG_MODULE_CACHE_PATH="$SWIFT_CACHE"

/usr/bin/swiftc -parse-as-library \
  -target "$BUILD_ARCH-apple-macos$DEPLOYMENT_TARGET" \
  "$PROJECT_DIR/StatusHost/StatusHost.swift" \
  "$DITCHD_LIBRARY" \
  -framework Cocoa \
  -framework UserNotifications \
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
  <string>Ditch Runtime</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>1.0.0</string>
  <key>CFBundleVersion</key>
  <string>1</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>LSUIElement</key>
  <true/>
  <key>LSMultipleInstancesProhibited</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
</dict>
</plist>
PLIST

cp "$DITCH_CLI_SOURCE" "$HELPER_MACOS/ditch_cli"
cp "$DITCH_CLI_SOURCE" "$MAIN_MACOS/ditch_cli"
# The remote artifact must be the portable Rust daemon, not the macOS status
# host wrapper. Its target-qualified name prevents accidental cross-arch use.
cp "$DITCHD_SOURCE" "$MAIN_MACOS/ditchd-remote-$HOST_REMOTE_TARGET"
cp "$DITCHD_SOURCE" "$HELPER_MACOS/ditchd-remote-$HOST_REMOTE_TARGET"

# Release automation may stage the cross-built Linux/macOS matrix produced by
# scripts/build-remote-artifacts. Keep these as checksum-verified resources;
# the host-native sibling above remains available for ordinary development.
WORKSPACE_VERSION=$(sed -n 's/^version = "\([^"]*\)"/\1/p' "$WORKSPACE_ROOT/Cargo.toml" | head -n 1)
REMOTE_ARTIFACT_SOURCE="${DITCH_REMOTE_ARTIFACT_DIR:-$WORKSPACE_ROOT/dist/remote/$WORKSPACE_VERSION}"
if [ -f "$REMOTE_ARTIFACT_SOURCE/remote-artifacts.json" ]; then
  ARTIFACT_BUILD_IDENTIFIER=$(sed -n 's/.*"build_identifier": "\([^"]*\)".*/\1/p' "$REMOTE_ARTIFACT_SOURCE/remote-artifacts.json" | head -n 1)
  if [ "$ARTIFACT_BUILD_IDENTIFIER" != "$DITCH_BUILD_IDENTIFIER" ]; then
    echo "error: remote runtime artifacts are stale for this source build." >&2
    echo "Run scripts/build-remote-artifacts before building the macOS app." >&2
    exit 1
  fi
  mkdir -p "$HELPER_RESOURCES"
  cp "$REMOTE_ARTIFACT_SOURCE/remote-artifacts.json" "$HELPER_RESOURCES/remote-artifacts.json"
  for ARTIFACT in "$REMOTE_ARTIFACT_SOURCE"/ditchd-*; do
    [ -f "$ARTIFACT" ] || continue
    cp "$ARTIFACT" "$HELPER_RESOURCES/$(basename "$ARTIFACT")"
    chmod 600 "$HELPER_RESOURCES/$(basename "$ARTIFACT")"
  done
fi

# Verify the copies before signing (codesign changes Mach-O bytes). `cmp` is
# killed by Xcode's script sandbox for Mach-O inputs, so verify executability
# and exact byte size here; `cp` itself already exits nonzero on write failure.
test -x "$MAIN_MACOS/ditch_cli"
test -x "$HELPER_MACOS/ditchd"
test "$(stat -f %z "$DITCH_CLI_SOURCE")" = "$(stat -f %z "$MAIN_MACOS/ditch_cli")"

# Every executable inside a notarized app must carry a hardened signature.
# During an Archive, use the identity selected by Xcode; local unsigned builds
# fall back to an ad-hoc identity while retaining Hardened Runtime flags.
SIGNING_IDENTITY="${EXPANDED_CODE_SIGN_IDENTITY:--}"
if [ -z "$SIGNING_IDENTITY" ]; then
  SIGNING_IDENTITY="-"
fi
/usr/bin/codesign --force --sign "$SIGNING_IDENTITY" --options runtime "$MAIN_MACOS/ditch_cli"
/usr/bin/codesign --force --sign "$SIGNING_IDENTITY" --options runtime "$MAIN_MACOS/ditchd-remote-$HOST_REMOTE_TARGET"
/usr/bin/codesign --force --sign "$SIGNING_IDENTITY" --options runtime "$HELPER_MACOS/ditchd-remote-$HOST_REMOTE_TARGET"
/usr/bin/codesign --force --sign "$SIGNING_IDENTITY" --options runtime "$HELPER_MACOS/ditch_cli"
/usr/bin/codesign --force --sign "$SIGNING_IDENTITY" --options runtime "$HELPER_APP"

rm -rf "$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/Library/LoginItems/The Ditch Status.app"
rm -f "$TARGET_BUILD_DIR/$CONTENTS_FOLDER_PATH/MacOS/ditch-status-host"
