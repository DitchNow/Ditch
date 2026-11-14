#!/bin/sh
# Package the optional macOS 15+ engine; no upstream app or agent executable.
set -eu
SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
COMMUNITY_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/../../.." && pwd)
DESTINATION=${1:?helper Contents directory required}
BUILD_ROOT=${2:?build directory required}
BUILD_ARCH=${3:?architecture required}
CONFIGURATION=${4:-release}
CARGO_MANIFEST=${5:-$COMMUNITY_ROOT/Cargo.toml}
PIN=70dbc4183f390a5a72d5dc01d8a77eb672217070
[ "$(git -C "$COMMUNITY_ROOT/vendor/peekaboo" rev-parse HEAD)" = "$PIN" ] || {
  echo 'error: initialize the pinned community/vendor/peekaboo submodule' >&2; exit 1;
}
case "$BUILD_ARCH" in
  arm64) CARGO_NATIVE_TARGET=aarch64-apple-darwin;;
  x86_64) CARGO_NATIVE_TARGET=x86_64-apple-darwin;;
  *) echo 'error: unsupported native architecture' >&2; exit 1;;
esac
case "$CONFIGURATION" in debug|release) ;; *) exit 1;; esac
PACKAGE="$SCRIPT_DIR/DitchAutomation"
mkdir -p "$BUILD_ROOT"
swift build --package-path "$PACKAGE" --scratch-path "$BUILD_ROOT" \
  --configuration "$CONFIGURATION" --arch "$BUILD_ARCH" \
  --force-resolved-versions --product DitchAutomation
BIN=$(swift build --package-path "$PACKAGE" --scratch-path "$BUILD_ROOT" \
  --configuration "$CONFIGURATION" --arch "$BUILD_ARCH" --show-bin-path)
FRAMEWORK="$DESTINATION/Frameworks/DitchAutomation.framework"
mkdir -p "$FRAMEWORK/Versions/A/Resources"
cp "$BIN/libDitchAutomation.dylib" "$FRAMEWORK/Versions/A/DitchAutomation"
install_name_tool -id '@rpath/DitchAutomation.framework/Versions/A/DitchAutomation' "$FRAMEWORK/Versions/A/DitchAutomation"
ln -sfn A "$FRAMEWORK/Versions/Current"
ln -sfn Versions/Current/DitchAutomation "$FRAMEWORK/DitchAutomation"
ln -sfn Versions/Current/Resources "$FRAMEWORK/Resources"
cat > "$FRAMEWORK/Resources/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>ai.theditch.automation</string>
<key>CFBundleExecutable</key><string>DitchAutomation</string>
<key>CFBundlePackageType</key><string>FMWK</string>
<key>CFBundleVersion</key><string>1</string>
<key>CFBundleShortVersionString</key><string>1.0</string>
<key>LSMinimumSystemVersion</key><string>15.0</string>
</dict></plist>
PLIST
python3 "$COMMUNITY_ROOT/scripts/collect-desktop-tool-licenses.py" \
  --checkouts "$BUILD_ROOT/checkouts" --destination "$FRAMEWORK/Resources" \
  --cargo-manifest "$CARGO_MANIFEST" \
  --target "$CARGO_NATIVE_TARGET"
