# Inline agent approvals

Approval requests now appear in the owning agent's chat, with **Approve in this
session**, **Cancel**, and **Approve once**. Cancel denies the pending operation;
it does not stop the agent. Commands and targets remain visible before choosing.
Agent question forms retain their existing flow.

Desktop implementation lives in the shared Community Flutter application, so both
desktop editions use it. Permission events and snapshots attach cards to the
matching project and agent. Cards remain visible if chat history cannot load.
Existing runtime commands preserve local and SSH execution routing and permission
scope. No local runtime policy or process management changes are introduced.

Mobile cards use Mac-owned attention requests and query their details over the
existing encrypted channel. The new `approve_session` decision maps to the Mac's
existing `ApprovePermissionForSession` handling. Owner authentication remains
required. Relay delivery does not imply approval: Mobile waits for the encrypted
Mac runtime result. Unknown results block duplicate submissions; request queries,
snapshots, and reconnects reconcile stale controls. Resolved cards are retained
in the current application's chat state, not written into the agent prompt or
added as a new durable transcript format. Pending requests restore from runtime
state after relaunch.

The architecture remains:

```text
Mobile ↔ Relay ↔ Mac ditchd ↔ SSH ↔ Remote ditchd ↔ Agents
```

## Applying the separate repository patches

Desktop and Mac runtime source changes are already in this workspace. Mobile was
implemented and tested in `build/remote-recovery/TheDitchMobile`, merged with the
current Mobile checkout's uncommitted startup, prompt, status and timeout changes.
The patches below are incremental against the separate checkouts as inspected
on September 12, 2026. They do not replace the earlier remote-recovery patches.
The original Mobile and Relay checkouts have not been edited by this task.

Apply Mobile:

```sh
cd "/Users/mtn/Documents/Personal/SentinelProjects/TheDitchMobile"
git apply --check "/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/patches/mobile-inline-approvals.patch" &&
git apply "/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/patches/mobile-inline-approvals.patch"
```

Optionally synchronize the Relay's contract documentation:

```sh
cd "/Users/mtn/Documents/Personal/SentinelProjects/TheDitchRelay"
git apply --check "/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/patches/relay-inline-approvals-contract.patch" &&
git apply "/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/patches/relay-inline-approvals-contract.patch"
```

The Relay patch changes only the contract document and its checksum manifest.
**No Relay runtime deployment or migration is required.** Rebuild/update the Mac
application and daemon before using session approval in the rebuilt Mobile app.
Old Mobile clients' `approve` and `deny` values remain supported. No remote host
setup, enrollment, key, or lingering changes are required for this feature.

`patches/inline-approvals-manifest.json` records before/after file hashes and patch
checksums. If an apply check fails after further source edits, merge the patch;
do not overwrite the checkout with the build copy.

## Verification

- Shared desktop and commercial desktop Flutter analysis and test suites.
- Mobile analysis and full test suite, including the current checkout's newer
  startup, request timeout, session status and prompt tests.
- Approval tests cover all three decisions, duplicate clicks, navigation,
  disconnected and expired requests, uncertain outcomes, cross-device resolution,
  history-load failure, and encrypted Mac acknowledgement versus Relay delivery.
- Mac daemon test suite, including authenticated routing of all three decisions
  to the owning SSH host.
- Incremental patches pass `git apply --check` against the separate checkouts.

These are source-level and automated checks. No app was deployed or installed,
and no physical iPhone-to-SSH acceptance session was run during this task.
