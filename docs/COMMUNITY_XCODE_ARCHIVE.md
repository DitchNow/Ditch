# Community archive workflow

Run these scripts from the private client workspace, just like `production.sh`
and `staging.sh` for Commercial:

```sh
./community-production.sh 0.1.0 122
# Or, for staging:
./community-staging.sh 0.1.0 122
```

Quit Xcode first. Commit the Community source changes before preparing the archive;
ignored local dotenv files remain on the release machine. The Community archive
uses its own Git revision and does not require updating the Commercial gitlink.

The scripts:

1. Read Community's selected `.env` and public verification configuration, and
   verify that its keys agree with the corresponding Commercial upgrade keys.
2. Read only Flutter and Developer ID certificate selections from the existing
   private root `.env.release-secrets.<environment>` configuration.
3. Set Community edition, app version, build number, release sequence, and the
   local `DITCH_COMMUNITY_BUILD_SEQUENCE` to the requested build.
4. Create or reuse a Community-specific credential and registration receipt.
5. Build all four Community remote runtimes and verify their manifest identity.
6. Generate Community's Flutter/Xcode configuration using `--config-only` and
   open `community/apps/macos/macos/Runner.xcworkspace` in a fresh Xcode process.

In Xcode, select **Product → Archive**, then follow the same **Developer ID → Upload**
workflow used for Commercial. Export the resulting Community app and put it in a
DMG. Publishing remains a separate Relay step; these scripts do not register,
notarize, upload, or publish a release themselves.

The supplied numbers are used literally: `0.1.0 122` produces version `0.1.0`,
build `122`, edition `community`. Relay now supports edition-scoped build numbers;
Community 122 can coexist with Commercial 122. Moving from Community to Commercial
still requires a compatible Commercial target with a higher build number.

## Credentials and Relay registration

Files are stored outside both Git repositories:

```text
~/Library/Application Support/DitchNow/ReleaseKeys/production/community/
  official-build-0.1.0-122.b64
  official-build-0.1.0-122.registration.json
```

Staging uses the corresponding `staging/community/` directory. Both files have mode
`0600`. Retrying the same source/version/build reuses them. A receipt that belongs
to another source revision or credential is rejected instead of overwritten.
Never reuse the Commercial build credential for Community.

These paths and the receipt format match Relay's `scripts/community-macos-release.mjs`.
That script currently supports registering an exported app without rebuilding it:

```sh
# Run from the Relay repository; replace the exported app path.
node scripts/community-macos-release.mjs production 0.1.0 122 \
  --register-only --app '/path/to/exported/Ditch.app'
```

Use the Relay publication workflow after registration. The registration helper
itself does not upload a DMG or publish Community update metadata. It checks that
the app contains the expected credential and identity before registering its hash.

Official Community upgrades require both the embedded registered build credential
and the public verification keys. A paid or complimentary license must also grant
Commercial access; merely running a Community build does not grant a license.

## OSS separation

No credential or dotenv file is added to the OSS repository. These maintainer
wrappers reside in the private client workspace. OSS contributors can continue
using the direct Flutter source-build commands without maintainer configuration,
Apple signing credentials, or a Relay credential. Official releases use the
existing protected release inputs explicitly.

## Validation

```sh
node --test scripts/test-prepare-community-archive.mjs
python3 community/scripts/test-macos-runtime-signing.py
```

Preparation tests substitute the compiler and Xcode launch; they verify environment/edition
selection, credential isolation, receipt compatibility and reuse, source mismatch
rejection, public trust matching, and remote artifact validation. They do not build
or publish an actual application.

On macOS, the signing regression tests compile ARM and Intel Mach-O fixtures and
run the archive's signing step with an ad-hoc identity. They verify Hardened
Runtime signatures, the helper's resource seal, refreshed runtime checksums,
unchanged Linux artifacts, and source builds without release credentials.

If Xcode reports **Hardened Runtime is Not Enabled** for the bundled
`ditchd-*-apple-darwin` resources, create a fresh archive with the corrected
Community packaging script. Retrying distribution of an existing archive does
not run the packaging script again. Commit the source fix first and use a new
build number: an existing registration receipt is tied to its original source
revision.
