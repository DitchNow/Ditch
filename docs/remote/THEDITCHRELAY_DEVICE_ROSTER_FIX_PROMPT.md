# Fix TheDitchRelay Machine Device Roster Contract

Work in this repository:

```text
/Users/mtn/Documents/Personal/SentinelProjects/TheDitchRelay
```

Do not edit The Ditch Mac repository or TheDitchMobile for this task.

## Confirmed failure

The Mac successfully authenticates to the production relay and calls:

```text
GET /v1/machines/{machine_id}/devices
```

The Mac then rejects the response with:

```text
remote_device_sync_failed: relay device omitted state
```

The failure is confirmed in `src/worker.ts`. The current `listMachineDevices` query selects only:

```text
d.id
d.name
d.platform
d.key_version
md.created_at AS authorized_at
md.last_seen_at
```

It omits these security-critical fields required by Remote Protocol v1:

```text
d.state
d.signing_public_key
d.agreement_public_key
```

The existing relay test does not catch this because it uses a partial `toMatchObject` assertion containing only the device ID and name.

## Required response contract

For every active phone authorized for the requested Mac, return exactly the following fields:

```json
{
  "protocol_version": 1,
  "machine_id": "22222222-2222-4222-8222-222222222222",
  "devices": [
    {
      "id": "11111111-1111-4111-8111-111111111111",
      "name": "iPhone",
      "platform": "ios",
      "state": "active",
      "signing_public_key": "base64url-p256-sec1-public-key",
      "agreement_public_key": "base64url-p256-sec1-public-key",
      "key_version": 1,
      "authorized_at": 1787596700000,
      "last_seen_at": 1787596800000
    }
  ]
}
```

`last_seen_at` may be `null`. All timestamps must be Unix epoch milliseconds represented as JSON integers, never RFC3339 strings.

The query should include at least:

```sql
SELECT
  d.id,
  d.name,
  d.platform,
  d.state,
  d.signing_public_key,
  d.agreement_public_key,
  d.key_version,
  md.created_at AS authorized_at,
  md.last_seen_at
FROM machine_devices md
JOIN devices d
  ON d.owner_id = md.owner_id
 AND d.id = md.device_id
WHERE md.owner_id = ?
  AND md.machine_id = ?
  AND md.state = 'active'
  AND d.state = 'active'
ORDER BY md.created_at, d.id
```

Adapt formatting to the existing codebase, but do not weaken the owner and machine scoping.

## Authorization invariants

Preserve all of the following:

- Only the authenticated machine may request its own roster.
- The machine must have an active owner relationship.
- Return only devices with an active `machine_devices` association for that exact machine.
- A phone owned by the same owner but not associated with this Mac must not appear.
- A revoked device or revoked machine-device association must not appear.
- Never rely on UUID secrecy.
- Do not replace this endpoint with an owner-wide device list.

## Tests to add or strengthen

Update the relay tests so they assert the complete response rather than a permissive subset.

Cover:

1. An authorized active phone returns every required field.
2. `state` is exactly `active`.
3. Both stored P-256 public keys are returned unchanged.
4. `key_version` is a positive integer.
5. `authorized_at` and non-null `last_seen_at` are JSON integers.
6. A null `last_seen_at` remains JSON `null`.
7. A same-owner phone without a `machine_devices` association is excluded.
8. A phone associated with another Mac is excluded.
9. Revoked devices and revoked associations are excluded.
10. A different owner cannot query the roster, even when all IDs are known.
11. A machine cannot query another machine's roster.

Use an exact assertion for each returned device object so future required-field omissions fail CI. Do not mock away D1 behavior.

## Contract consistency

Compare the implementation with the canonical contract in the Mac repository:

```text
/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/openapi-v1.yaml
/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/REMOTE_PROTOCOL_V1.md
/Users/mtn/Documents/Personal/The Ditch v2/docs/remote/fixtures/v1.json
```

Do not modify those canonical files from the relay repository. If the relay keeps a copied/exported contract, refresh it through the existing contract export/import mechanism and verify its checksum.

## Quality gate

Run from TheDitchRelay:

```bash
npm run check
```

This must pass contract verification, TypeScript checking, Worker tests, D1 tests, and Node tests.

Also run the focused lifecycle/device-roster tests separately and report their exact results.

## Production deployment

After tests pass, deploy the production Worker using the repository's existing command:

```bash
npm run deploy:production
```

Do not treat a Docker-local run as a production deployment. The Mac uses this built-in origin unless explicitly overridden:

```text
https://relay.ditchnow.nl
```

Confirm that the deployed Worker version and production D1 binding are the intended environment. Do not print secrets or authentication material.

## Production verification

After deployment:

1. Open Remote Control settings on the Mac and trigger a device refresh.
2. Confirm the error `relay device omitted state` no longer appears.
3. Confirm only phones authorized for that Mac are displayed.
4. Confirm revoking or disconnecting a phone removes it from the next successful refresh.
5. Inspect privacy-safe Worker logs for the request ID and status only; do not log public-key material or sensitive payloads unnecessarily.

If production verification cannot be performed because credentials or an authenticated machine are unavailable, state that explicitly. Do not claim the fix is deployed or verified without evidence.

## Non-goals

Do not:

- weaken the Mac's device validation;
- return every device owned by the owner;
- expose private keys;
- add login, team, or RBAC features;
- modify pairing semantics;
- add an offline command queue;
- edit the Mac or iPhone repositories.

## Final report

Report:

1. Root cause.
2. Files changed.
3. Exact response fields now returned.
4. Owner/machine isolation behavior.
5. Tests run and actual results.
6. Whether production deployment actually succeeded.
7. Whether the live Mac refresh was actually verified.
8. Any remaining incompatibility with the canonical Remote Protocol v1.
