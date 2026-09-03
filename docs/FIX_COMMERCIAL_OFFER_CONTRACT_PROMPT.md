# Codex Implementation Prompt — Repair the Relay/macOS Commercial Offer Contract

> Private Commercial implementation material. Keep this document in the Commercial repository.

## Mission

Work in both existing repositories:

```text
/Users/mtn/Documents/Personal/The Ditch v2
/Users/mtn/Documents/Personal/SentinelProjects/TheDitchRelay
```

Fix the live Commercial pricing failure by making the Ditch Relay and the macOS Community upgrade client use one exact, provider-neutral Commercial-offer wire contract.

Implement and test the fix in both repositories. Do not stop after inspection.

Do not deploy the Relay, publish artifacts, create a release, or modify live Stripe/Cloudflare state unless the user separately and explicitly authorizes deployment.

## Observed Failure

The macOS Remote Control dialog currently displays:

```text
Pricing is temporarily unavailable.
```

The running `ditchd` reports the exact underlying failure:

```text
commercial_offers_failed: upgrade service response is invalid:
Failed to read JSON: invalid type: integer `1788085651552`, expected a string
```

The live Relay is reachable. Machine registration, Commercial owner bootstrap, authentication, and `GET /v1/commercial/entitlement` work. This is not the previous `api.ditchnow.nl` DNS failure.

The failure is caused by contract drift between:

```text
TheDitchRelay/src/billing/catalog.ts

and

The Ditch v2/crates/ditch_upgrade/src/lib.rs
The Ditch v2/apps/macos/lib/data/commercial_models.dart
```

The two repositories currently test different JSON representations, so both test suites can pass while the real integration fails.

## Architectural Constraints

Preserve the existing architecture:

```text
The Ditch macOS
  -> Community ditchd
  -> authenticated Ditch Relay API
  -> Stripe
```

Do not:

```text
add a Stripe SDK to macOS
call Stripe directly from macOS
expose Stripe Product, Price, Coupon, Customer, or Subscription IDs
hardcode prices in Flutter or Rust
infer Ditch entitlements from price or Product text
change Community/Commercial composition
add a second daemon
change local or SSH behavior
weaken catalog validation into accepting arbitrary JSON
bump the protocol major version merely to repair this unreleased v1 shape
```

Stripe controls commercial price, currency, billing cadence, active sale availability, Product presentation, and the supported introductory promotion. Ditch owns capability and entitlement meaning.

## Canonical v1 Contract

Treat this Community fixture as the canonical protocol-v1 representation:

```text
/Users/mtn/Documents/Personal/The Ditch v2/docs/contracts/commercial-offers-v1.json
```

The response from:

```http
GET /v1/commercial/offers
```

must have this shape:

```json
{
  "protocol_version": 1,
  "stale": false,
  "refreshed_at": "2026-08-30T08:00:00Z",
  "offers": [
    {
      "offer_id": "opaque-relay-owned-id",
      "kind": "commercial_monthly",
      "title": "Commercial Monthly",
      "description": "Ditch Commercial Remote Control",
      "currency": "EUR",
      "base_amount_minor": "1500",
      "minor_unit_exponent": 2,
      "billing_type": "recurring",
      "recurring_interval": "month",
      "recurring_interval_count": 1,
      "introductory_price": {
        "amount_minor": "750",
        "duration_count": 3,
        "duration_unit": "month"
      },
      "purchase_action": "acquire",
      "eligible": true,
      "ineligible_reason": null,
      "entitlement": {
        "mac_slots": 1,
        "iphone_slots": 1,
        "ssh_hosts_unlimited": true
      }
    }
  ]
}
```

Requirements:

- `refreshed_at` is an RFC 3339 UTC string, not epoch milliseconds.
- Monetary amounts are exact, non-negative base-10 strings in minor units. Never use floating-point money arithmetic.
- `minor_unit_exponent` is explicit and correct for the returned currency. Do not blindly assume every currency has exponent 2. Fail a malformed or unsupported catalog entry closed.
- A standard offer uses `"introductory_price": null`.
- A paid introductory offer uses the nested object shown above. Do not expose separate flattened introductory fields.
- The entitlement object always uses the same field names for every offer kind. Its values are Ditch-owned display semantics, not Stripe metadata.
- Provider IDs and provider branding remain absent.
- Preserve owner-aware `eligible`, `ineligible_reason`, `purchase_action`, and offer visibility policy.

Current Relay fields that must not remain in the public response include:

```text
normal_unit_amount
introductory_unit_amount
introductory_duration
introductory_duration_unit
macs_per_unit
iphones_per_unit
macs
iphones
ssh_remote_nodes
numeric refreshed_at
```

Internal Relay/D1/provider models may retain appropriate internal names. Only the provider-neutral API serializer must conform to the canonical contract.

## Relay Work

Inspect the current uncommitted Relay implementation before editing, particularly:

```text
src/billing/catalog.ts
src/billing/provider.ts
src/billing/commercial.ts
src/worker.ts
test/commercial.test.ts
test/commercial_catalog.test.ts
test/stripe_provider.test.ts
migrations/0009_dynamic_commercial_catalog.sql
docs/COMMERCIAL_BILLING.md
```

Then:

1. Change the `GET /v1/commercial/offers` serializer to emit the canonical v1 shape exactly.
2. Convert the stored catalog refresh epoch to an RFC 3339 UTC response string without changing durable timestamp arithmetic unnecessarily.
3. Expose exact minor-unit amounts under the canonical names.
4. Supply and validate the correct currency minor-unit exponent.
5. Normalize introductory pricing into `introductory_price`.
6. Normalize Ditch entitlement display summaries into `mac_slots`, `iphone_slots`, and `ssh_hosts_unlimited` for Monthly, Lifetime, and Extra Pair.
7. Keep opaque offer-ID resolution and checkout revalidation unchanged in security meaning.
8. Preserve the existing route:

   ```http
   POST /v1/commercial/offers/{offer_id}/checkout
   ```

9. Confirm its successful response remains compatible with macOS:

   ```json
   {
     "protocol_version": 1,
     "checkout_intent_id": "uuid",
     "checkout_url": "https://hosted-checkout.example/...",
     "expires_at": 1788087000000
   }
   ```

10. Do not alter payment-proof, webhook, entitlement, refund, expiry, renewal, license, billing-management, or Commercial-artifact authorization semantics.
11. Update Relay documentation and tests to assert the canonical response rather than the obsolete Relay-only field names.

## macOS/Ditch Work

Inspect:

```text
crates/ditch_upgrade/src/lib.rs
crates/ditchd/src/main.rs
crates/ditch_protocol/src/lib.rs
apps/macos/lib/data/commercial_models.dart
apps/macos/lib/main.dart
apps/macos/test/widget_test.dart
docs/contracts/commercial-offers-v1.json
```

Then:

1. Keep the provider-neutral canonical field names already used by the Community client.
2. Validate `refreshed_at` as a real RFC 3339 timestamp when present, not merely an arbitrary string.
3. Preserve strict money, interval, introductory-price, eligibility, and entitlement validation.
4. Do not add permanent aliases for the Relay's broken/unreleased response fields unless a demonstrated released-client compatibility requirement exists.
5. Add a Rust deserialization/validation test using the exact canonical fixture.
6. Add or strengthen Flutter tests proving standard Monthly, paid introductory Monthly, Lifetime, and eligible Extra Pair render from Relay data with no locally hardcoded price.
7. Preserve the restrained generic user message on transient catalog failure and the Retry action.
8. Ensure a valid catalog clears the error and renders cards, including base-price strikethrough plus introductory price/duration where applicable.
9. Preserve active-subscriber behavior: ordinary acquisition cards disappear, while only Relay-authorized upgrade/add-capacity actions may appear.
10. Preserve checkout, `Finishing upgrade...`, entitlement polling, Manage Billing, license redemption, Commercial artifact verification, and single-`ditchd` behavior.

## Cross-Repository Contract Gate

Prevent another split-brain contract.

Add a deterministic cross-repository verification mechanism with these properties:

```text
one canonical protocol-v1 fixture/schema
Relay response tests validate against it
Rust deserialization validates it
Flutter parsing/rendering validates it
normal isolated builds do not require the neighboring private repository
private Relay CI can verify against an explicitly pinned/checked-out Community revision
```

Choose the least fragile implementation compatible with the repositories. A suitable approach is:

1. Keep the public canonical fixture and JSON Schema in Community.
2. Export/copy the generated contract into Relay with an explicit source revision and checksum.
3. Add a verification script that compares the Relay copy with a supplied `DITCH_COMMUNITY_ROOT` checkout and fails on drift.
4. Run that verification in Relay CI using an exact Community revision.

Do not make Community builds depend on the private Relay checkout. Do not rely on developers remembering to update two hand-written interfaces.

## Required Tests

Relay tests must cover:

```text
standard Monthly canonical response
paid introductory Monthly canonical response
Lifetime canonical response
Extra Pair canonical response and Lifetime eligibility
RFC 3339 refreshed_at
correct minor-unit exponent
no provider identifiers
archived/replacement Price behavior
stale catalog response
unavailable catalog failure
opaque offer checkout binding
checkout response compatibility
```

Ditch tests must cover:

```text
canonical fixture deserializes through ditch_upgrade
Relay catalog traverses local IPC as CommercialOffers
standard Monthly rendering
introductory strikethrough and duration rendering
Lifetime rendering
Extra Pair visibility policy
invalid numeric refreshed_at fails clearly
missing/invalid minor amount or exponent fails closed
catalog failure shows generic Retry UI
entitlement remains independently usable
checkout uses opaque offer_id unchanged
```

Run, at minimum:

```sh
# Relay
npm test
npm run typecheck

# Ditch
cargo test --workspace
cargo clippy --workspace --all-targets
cd apps/macos
flutter analyze
flutter test
```

Also run the Community leakage gate and any existing targeted contract/deployment validation scripts. Do not claim commands passed if sandbox or toolchain limitations prevented them; report exact blockers.

## Manual Acceptance

After local tests pass, report that deployment still requires authorization. Once the user separately authorizes and performs a Relay deployment, acceptance is:

```text
1. Open Settings -> Remote Control in Community Ditch.
2. Pricing cards load from the live Relay.
3. The €15 Monthly base price and eligible €7.50-for-three-month introductory price render correctly from Stripe/Relay data.
4. Lifetime renders from Relay data.
5. No price is hardcoded in macOS.
6. Selecting an offer opens the hosted checkout URL.
7. Browser return alone does not activate Commercial.
8. Relay entitlement activation remains authoritative.
9. Active subscribers no longer see ordinary acquisition plans.
10. Local and SSH Community functionality is unchanged.
```

## Final Report

Report only actual findings and changes:

1. exact root cause;
2. final canonical response contract;
3. Relay serializer/provider changes;
4. Ditch parser/UI changes;
5. cross-repository drift gate;
6. tests run and exact results;
7. files changed in each repository;
8. remaining deployment/manual acceptance steps.

Do not claim the live issue is resolved until the compatible Relay build is actually deployed and verified by a real authenticated macOS catalog request.
