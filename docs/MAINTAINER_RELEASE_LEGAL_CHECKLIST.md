# Maintainer release and legal checklist

This checklist is operational guidance, not legal advice. Final contribution-licensing terms and the Commercial distribution license require review by qualified counsel.

## Before merging Community contributions

- Confirm Community remains `AGPL-3.0-only`.
- Do not merge external source into a release branch until qualified counsel has approved and maintainers have published the inbound licensing and sign-off workflow.
- Do not infer proprietary distribution permission from AGPL licensing, a pull-request submission, or a DCO sign-off alone.
- Confirm no separate CLA is being represented as required unless DitchNow later adopts one through an approved policy change.
- Confirm the pull request contains no Commercial implementation, private protocol contract, private artifact, production Relay logic, secret, or credential.
- Run the Community leakage gate against the exact merge commit.

## Community release

- Build from a clean public checkout with no private repository, private registry, Git dependency, neighboring checkout, or secret except Apple release credentials.
- Run Rust and Flutter build, test, lint, and analyze jobs.
- Run `scripts/check-community-leakage` against source, Rust binaries, `.app`, and DMG contents.
- Confirm exactly one local runtime helper named `ditchd` and no secondary service daemon.
- Confirm remote SSH artifacts declare `edition: community` and match the exact source revision, protocol, build identifier, and digest.
- Sign with the approved Apple Developer ID, notarize, staple, install on a clean Mac, and test rollback/failure behavior.
- Publish only the Community DMG through the public release channel.

## Commercial release

- Verify the private build pins the intended exact Community commit and has no dirty submodule state.
- Run Community and Commercial test suites, state-compatibility fixtures, entitlement-expiry tests, and the single-runtime assertion.
- Confirm the Commercial app and every Commercial SSH artifact contain one authoritative `ditchd`, not an additional daemon.
- Generate and sign the durable release descriptor; verify release ID/channel, edition, version, build, compatibility, source revision, digest, size, bundle ID, Team ID, publication time, and rollback sequence. Separately verify Relay issues only short-lived entitlement-bound update sessions.
- Sign, notarize, staple, and test the in-place upgrade while preserving Application Support, preferences, Keychain, Codex state, projects, sessions, and SSH registrations.
- Upload Commercial artifacts only to private storage and register them with the entitlement backend. Never attach them to the public GitHub release.

## Manual/legal items

- Obtain legal approval for the final no-separate-CLA inbound licensing and sign-off workflow before merging external source into release branches.
- Confirm whether the statically composed proprietary Commercial build may consume each Community revision; do not assume free availability of a contributed feature supplies proprietary distribution rights.
- Obtain legal approval for the proprietary Commercial end-user/distribution terms.
- Review pricing, tax, refund, privacy, consumer, and subscription disclosures for supported markets.
- Review backend, billing processor, Relay, push notification, and artifact-retention data handling before production launch.
