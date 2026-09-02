# macOS deployment environments

These committed dotenv files contain public build configuration only:

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

Secure Commercial installation also requires four public verification values.
Copy `.env.release.example` to `.env.release.staging` or
`.env.release.production` and set the public Sparkle Ed25519 key, public P-256
manifest key, Apple Team ID, and a positive globally increasing build sequence.
The corresponding private signing keys never belong in this repository or the
macOS application.

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
