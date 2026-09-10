# Apply the complimentary license implementation

This implementation was built and tested in a temporary Relay copy because the
Relay repository is outside this session's writable folders. The live Relay source,
production deployment, and production license database have not been changed.

From the Relay repository, apply the patch:

```sh
cd '/Users/mtn/Documents/Personal/SentinelProjects/TheDitchRelay'
git apply --check '/Users/mtn/Documents/Personal/The Ditch v2/relay-complimentary-licenses.patch'
git apply '/Users/mtn/Documents/Personal/The Ditch v2/relay-complimentary-licenses.patch'
```

Then follow the deployment and key-creation steps below. The existing deployment
script applies the database migration before publishing the Worker. No new secret
is needed: the admin flow uses the environment's existing `INTERNAL_ADMIN_SECRET`.

Verification: `npm run check` passed (149 Worker tests, 12 deployment tests,
4 admin command tests), including TypeScript and protocol contract verification.
Tests used local Cloudflare fixtures; live staging/production acceptance is still
required. No real purchases or licenses were created during testing.

# Complimentary Commercial licenses

Admins can issue a Ditch key without a Stripe purchase. The recipient enters it in
**License & Plans → Activate license key**. The existing signed redemption route
claims the grant for that Owner; normal Mac activation, remote-control authorization,
and private Commercial update authorization then apply.

Defaults are **2 Macs, 2 iPhones, unlimited SSH remote nodes, and no expiry**.
Use an absolute UTC expiry for testers. Expiry is measured from the specified date,
not from redemption. Each key belongs to one Owner after its first claim; it is not
a reusable coupon for multiple customers. Issue a separate key for each recipient.

## Deploy before issuing

Apply migration `0015_complimentary_licenses.sql` before deploying the new Worker.
The normal deployment script already applies pending migrations before publishing:

```sh
npm run check
./scripts/deploy-cloudflare staging
# Verify issuance, app redemption, remote control, and revocation in staging.
./scripts/deploy-cloudflare production
```

The migration is additive. Existing Stripe keys, purchases, subscriptions, and
administrative grants are preserved. Complimentary keys are environment-specific;
a staging key cannot activate production.

## Create a production key

Run from the Relay repository. `--secret-file` takes the file configured as
`INTERNAL_ADMIN_SECRET_FILE` in that environment's `.env` file, not the secret itself.
With the current production configuration:

```sh
node scripts/complimentary-license-admin.mjs issue production \
  --secret-file ./.secrets/internal_admin_secret \
  --reason 'Personal use' \
  --output "$HOME/Desktop/ditch-personal-license.json"
```

The output file has mode `0600`. Open it and copy **`license.license_key`** into the
app's activation field. Retain **`license.license_id`** for lookup or revocation.
Keep this file private; it contains an activation credential. The command does not
print the key or admin token and refuses to overwrite an existing output file.

A time-limited staging tester example:

```sh
node scripts/complimentary-license-admin.mjs issue staging \
  --secret-file ./.secrets/internal_admin_secret.staging \
  --reason 'Tester: September acceptance' \
  --mac-slots 1 --ios-slots 1 \
  --expires-at 2026-10-10T23:59:59Z \
  --output "$HOME/Desktop/ditch-tester-license.json"
```

Choose a future expiry. Both device limits must be integers from 1 through 1000.
A grant adds its capacity to any other active grants or paid products. It does not
cancel or change an existing subscription. Active complimentary access hides new
acquisition offers; it does not qualify as a paid Lifetime product for add-ons.

## Recover an interrupted issuance

Before contacting Relay, the command saves the environment and exact request in
its output file. If the connection fails, the grant may already have been created.
Repeat the same command and parameters with the saved `request.license_id` supplied
as `--request-id`, and use a new `--output` filename. The same key is returned.
Changing the reason, limits, or expiry while reusing that ID returns a conflict.
There is no automatic retry that could issue a second grant.

## Inspect or revoke

Replace the example UUID with the saved license ID:

```sh
node scripts/complimentary-license-admin.mjs show production \
  --secret-file ./.secrets/internal_admin_secret \
  --license-id 22222222-2222-4222-8222-222222222222

node scripts/complimentary-license-admin.mjs revoke production \
  --secret-file ./.secrets/internal_admin_secret \
  --license-id 22222222-2222-4222-8222-222222222222
```

Show returns metadata (reason, limits, expiry, claim owner, and lifecycle state),
never the key. `claimed` remains the lifecycle state after time expiry; inspect
`expires_at` for the deadline. Revocation is permanent and safe to repeat. It
removes only this grant; other active paid or complimentary sources remain valid.
Existing device associations are retained. Without another usable source, new
remote-control operations and Commercial download requests are denied. An already
received artifact cannot be recalled. The app refreshes its local entitlement
through its existing synchronization flow.

## HTTP contract and storage

All admin routes require `Authorization: Bearer <INTERNAL_ADMIN_SECRET>` and return
`Cache-Control: no-store`. Admin credentials never belong in the Mac app.

- `POST /v1/internal/commercial/complimentary-licenses` accepts a UUID v4 `license_id`,
  a nonempty `reason` (maximum 500 characters), optional `mac_slots`/`ios_slots`, and
  nullable `expires_at` (Unix milliseconds). New issuance returns 201, an identical
  retry returns 200, changed details return 409. Successful responses include the
  key. A revoked grant cannot be retrieved by replaying issuance.
- `GET /v1/internal/commercial/complimentary-licenses/{id}` returns metadata only.
- `POST /v1/internal/commercial/complimentary-licenses/{id}/revoke` revokes a grant.
- Existing `POST /v1/commercial/license/redeem` accepts either a complimentary key
  or a verified purchase key. Complimentary lookup never bypasses the paid key's
  original purchase verification. Recognized expired/revoked keys fail closed.

D1 stores HMAC lookup values and AES-GCM ciphertext bound to license ID and
environment, never plaintext keys. Issuance, claim, and revocation have transactional
audit events; notes are stored only in the grant metadata. A conditional claim and
an immutable-owner trigger prevent two owners from sharing the grant. Durable source
revisions prevent older entitlement projections from hiding a revocation. Expiry
refreshes permissions at the next deadline while retaining any remaining paid access.

Claimed complimentary owners can also obtain a stable owner recovery key through
the existing authenticated `/v1/commercial/license` endpoint. This does not add the
currently missing owner-key display/recovery UI to the Mac app. The complimentary
activation key itself is a single-owner claim credential, not a way to move an
already-owned Mac into someone else's Owner.
