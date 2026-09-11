# macOS deployment environments

Local dotenv files contain public build configuration only and are ignored by
Git:

- `.env.staging` targets the isolated staging Relay and its Stripe sandbox.
- `.env.production` targets the official production Relay and live Stripe data.

They must never contain Stripe keys, webhook secrets, Cloudflare tokens, Apple
signing credentials, license peppers, or other credentials. Those belong in the
Relay or the protected release environment.

Supported variables:

```text
DITCH_DEPLOYMENT_ENVIRONMENT=staging|production
DITCH_RELAY_ORIGIN=https://relay-host-without-a-path
DITCH_UPDATE_ALLOWED_HOSTS=comma-separated-hostnames
```

Secure Commercial installation also requires five public verification values.
Copy `release.example` to `.env.release.staging` or
`.env.release.production` and set the public Sparkle Ed25519 key, public P-256
Commercial manifest key, Community manifest key, Apple Team ID, and a positive
globally increasing build sequence. The Commercial key verifies an upgrade from
Community to Commercial; the Community key verifies Community updates.
The corresponding private signing keys never belong in this repository or the
macOS application.

Official release builds must set `DITCH_CODESIGN_IDENTITY` to their Developer ID
Application certificate name when running `scripts/macos-app`. The wrapper writes
this public certificate selection into the ignored `ReleaseVerification.xcconfig`.
A clean OSS checkout defaults to ad-hoc signing and requires neither a DitchNow
Apple team nor any dotenv file for direct `flutter run -d macos` / `flutter build macos`.
The official release workflow supplies its signing identity explicitly.

CI creates its environment files at runtime from protected environment values;
no `.env` file is committed to the repository. Both `.release-keys/` and `.secrets/`
are ignored, and the leakage gate rejects tracked files in those directories.
Private Relay administration, billing, and signing credentials never go into these
local app dotenv files. The official per-build credential is separate release input;
creating the public configuration alone does not register an official build with Relay.

Official DitchNow builds additionally require a per-build Relay credential.
Supply `DITCH_OFFICIAL_BUILD_CREDENTIAL` from protected CI, or put the raw
unpadded base64url value in the ignored mode-0600 file
`.release-keys/official-build-<environment>.token`. Source builds intentionally
omit it and cannot create a hosted-services session. Rotate the credential for
every published build and register only its hash with the matching Relay. The
packager removes the credential from Cargo's environment and injects it only
into the signed in-process macOS runtime host; standalone CLI and SSH runtime
artifacts remain credential-free.

Use the repository wrapper so Flutter, Xcode, the bundled Rust runtime, and the
native Commercial-update allowlist receive one consistent configuration:

```sh
scripts/macos-app staging run
scripts/macos-app production run
scripts/macos-app staging build
scripts/macos-app production build
```

A release build fails before Flutter starts when these verification values are
missing, malformed, or placeholders. A local `run` remains available for
Community development, but prints a warning that Commercial installation is
unavailable until the public trust configuration is supplied.

The wrapper rejects a staging build pointed at the production Relay and a
production build pointed anywhere except `https://relay.ditchnow.nl`. A direct
`flutter run -d macos` or `flutter build macos` retains the historical
production default for Community source-build compatibility.

Staging and production intentionally retain the same application identity,
local database, projects, Codex identity, and SSH configuration. Run one
environment at a time. When an app built for the other environment launches,
the AppKit shell stops and re-registers the single authoritative `ditchd` so it
cannot reconnect to a runtime compiled with the previous Relay origin. The
switch is refused while agents are active or their status cannot be verified;
environment switching never kills an active agent.
