#!/bin/sh
# Sourced by package_status_host.sh after assembling the runtime helper.
set -eu

# Every executable inside a notarized app must carry a hardened signature.
# During an Archive, use the identity selected by Xcode; local unsigned builds
# fall back to an ad-hoc identity while retaining Hardened Runtime flags.
SIGNING_IDENTITY="${EXPANDED_CODE_SIGN_IDENTITY:--}"
if [ -z "$SIGNING_IDENTITY" ]; then
  SIGNING_IDENTITY="-"
fi
sign_code() {
  if [ "$SIGNING_IDENTITY" = "-" ]; then
    /usr/bin/codesign --force --sign - --options runtime "$1"
  else
    /usr/bin/codesign --force --timestamp --sign "$SIGNING_IDENTITY" --options runtime "$1"
  fi
}

# Validate the copied manifest before changing any bytes. Sign both macOS
# resource binaries, then refresh only their hashes before sealing the helper.
# Linux artifacts remain byte-for-byte identical to the cross-built originals.
if [ -d "$HELPER_RESOURCES" ]; then
  MANIFEST="$HELPER_RESOURCES/remote-artifacts.json"
  ARTIFACT_COUNT=$(/usr/bin/plutil -extract artifacts raw -expect array "$MANIFEST")
  INDEX=0
  while [ "$INDEX" -lt "$ARTIFACT_COUNT" ]; do
    TARGET=$(/usr/bin/plutil -extract "artifacts.$INDEX.target" raw -expect string "$MANIFEST")
    ARTIFACT=$(/usr/bin/plutil -extract "artifacts.$INDEX.artifact" raw -expect string "$MANIFEST")
    case "$TARGET" in
      aarch64-apple-darwin|x86_64-apple-darwin|aarch64-unknown-linux-gnu|x86_64-unknown-linux-gnu) ;;
      *) echo "error: unsupported bundled runtime target: $TARGET" >&2; exit 1 ;;
    esac
    [ "$ARTIFACT" = "ditchd-$TARGET" ] || {
      echo "error: bundled runtime filename does not match its target" >&2; exit 1;
    }
    EXPECTED=$(/usr/bin/plutil -extract "artifacts.$INDEX.sha256" raw -expect string "$MANIFEST")
    ACTUAL=$(/usr/bin/shasum -a 256 "$HELPER_RESOURCES/$ARTIFACT" | awk '{print $1}')
    [ "$EXPECTED" = "$ACTUAL" ] || {
      echo "error: bundled runtime checksum mismatch: $ARTIFACT" >&2; exit 1;
    }
    INDEX=$((INDEX + 1))
  done
  for REMOTE_RUNTIME in "$HELPER_RESOURCES"/ditchd-*-apple-darwin; do
    [ -f "$REMOTE_RUNTIME" ] || continue
    sign_code "$REMOTE_RUNTIME"
    /usr/bin/codesign --verify --strict "$REMOTE_RUNTIME"
  done
  INDEX=0
  while [ "$INDEX" -lt "$ARTIFACT_COUNT" ]; do
    TARGET=$(/usr/bin/plutil -extract "artifacts.$INDEX.target" raw -expect string "$MANIFEST")
    case "$TARGET" in
      *-apple-darwin)
        CHECKSUM=$(/usr/bin/shasum -a 256 "$HELPER_RESOURCES/ditchd-$TARGET" | awk '{print $1}')
        /usr/bin/plutil -replace "artifacts.$INDEX.sha256" -string "$CHECKSUM" "$MANIFEST"
        ;;
    esac
    INDEX=$((INDEX + 1))
  done
  /usr/bin/plutil -convert json "$MANIFEST"
fi

sign_code "$MAIN_MACOS/ditch_cli"
sign_code "$MAIN_MACOS/ditchd-remote-$HOST_REMOTE_TARGET"
sign_code "$HELPER_MACOS/ditchd-remote-$HOST_REMOTE_TARGET"
sign_code "$HELPER_MACOS/ditch_cli"
sign_code "$HELPER_APP"
for SIGNED_CODE in \
  "$MAIN_MACOS/ditch_cli" \
  "$MAIN_MACOS/ditchd-remote-$HOST_REMOTE_TARGET" \
  "$HELPER_MACOS/ditchd" \
  "$HELPER_MACOS/ditch_cli" \
  "$HELPER_MACOS/ditchd-remote-$HOST_REMOTE_TARGET" \
  "$HELPER_APP"
do
  /usr/bin/codesign --verify --strict "$SIGNED_CODE"
done

