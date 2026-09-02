# Codex Prompt — Harden the Ditch macOS Commercial Offer Contract

> Private Commercial implementation material. Keep this document in the Commercial repository.

## Repository and Ownership

Work only in:

```text
/Users/mtn/Documents/Personal/The Ditch v2
```

You own the Community `ditchd` upgrade client, local IPC contract, Flutter parsing/UI, and Community-side contract tests. Do not edit or deploy the Relay repository.

Inspect the current implementation first, then implement and test the Community-side work. Do not stop after reconnaissance.

The Relay is being repaired independently to emit the canonical contract already represented by:

```text
docs/contracts/commercial-offers-v1.json
```

Do not change the canonical client to adopt the Relay's currently broken/unreleased field names.

## Problem

The Remote Control dialog currently displays:

```text
Pricing is temporarily unavailable.
```

The running Community `ditchd` reports:

```text
commercial_offers_failed: upgrade service response is invalid:
Failed to read JSON: invalid type: integer `1788085651552`, expected a string
```

The live Relay is reachable and `GET /v1/commercial/entitlement` succeeds. The immediate failure occurs while deserializing the Relay offer catalog.

The Relay currently emits an incompatible response using numeric refresh time, different money names, flattened introductory fields, and inconsistent entitlement field names. A separate Relay implementation task will repair its serializer.

The Community client must remain strict, provider-neutral, safe, and independently buildable.

## Architectural Constraints

Preserve:

```text
The Ditch.app
  -> one Community ditchd
  -> authenticated Ditch Relay API
  -> hosted Checkout URL
```

Do not:

```text
add Stripe SDKs or Stripe API calls
add Stripe keys, Price IDs, Customer IDs, or provider-specific models
hardcode prices or packages in Rust/Flutter
derive capabilities from price/title/description
add Remote Control implementation to Community
change Community/Commercial composition
add a second runtime daemon
change local projects, SSH projects, Codex, PTYs, or persistence
show fallback or zero placeholder prices
weaken parsing to accept arbitrary malformed catalog data
bump protocol v1 only for secrecy or naming drift
```

## Canonical Protocol-v1 Contract

Keep this fixture authoritative:

```text
docs/contracts/commercial-offers-v1.json
```

The client consumes:

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

Required semantics:

- `refreshed_at` is an RFC 3339 UTC string.
- Amounts are exact non-negative decimal strings in minor units.
- `minor_unit_exponent` controls locale-aware display and is never inferred from a floating-point value.
- Paid introductory pricing is one nested object associated with its base offer.
- Entitlement metadata is display-only Ditch domain state; the client does not infer it from price.
- `offer_id` is opaque and passes unchanged through Flutter, local IPC, `ditchd`, and Relay checkout.
- Provider object IDs and provider branding never enter Community models or UI.

Do not add permanent aliases for these broken Relay response fields unless there is evidence they shipped as a supported public contract:

```text
numeric refreshed_at
normal_unit_amount
introductory_unit_amount
introductory_duration
introductory_duration_unit
macs_per_unit
iphones_per_unit
macs
iphones
ssh_remote_nodes
```

## Implementation Scope

Inspect at least:

```text
crates/ditch_upgrade/src/lib.rs
crates/ditch_identity/src/lib.rs
crates/ditch_protocol/src/lib.rs
crates/ditchd/src/main.rs
apps/macos/lib/data/commercial_models.dart
apps/macos/lib/main.dart
apps/macos/test/widget_test.dart
docs/contracts/commercial-offers-v1.json
docs/MACOS_DYNAMIC_COMMERCIAL_OFFERS_IMPLEMENTATION_PROMPT.md
scripts/check-community-leakage
.github/workflows
```

Implement or strengthen:

1. Rust deserialization and validation of the canonical catalog fixture.
2. RFC 3339 parsing/validation of `refreshed_at` when present. Do not accept an arbitrary string merely because Serde can deserialize it.
3. Exact money validation, supported exponent bounds, recurrence consistency, and introductory amount/duration validation.
4. Entitlement field validation without deriving capability policy from Stripe-facing data.
5. Local IPC serialization of the validated catalog through `ServerResponse::CommercialOffers`.
6. Flutter parsing of the exact canonical fields.
7. Locale-aware price formatting from currency, minor-unit amount, and exponent.
8. Paid introductory presentation:

   ```text
   base price shown with strikethrough
   introductory price emphasized
   exact paid introductory duration shown
   normal recurring cadence shown accurately
   ```

9. Multiple active Relay offers rendering independently.
10. Relay-controlled eligibility, visibility, acquisition, future upgrade, renewal, and add-capacity presentation.
11. Existing-subscriber behavior:

   ```text
   active Commercial -> hide ordinary acquisition cards
   active Lifetime -> allow only Relay-returned Extra Pair/add-capacity
   future upgrades -> show only when Relay explicitly returns them
   expired subscription -> local/SSH continue and Relay-returned renewal may appear
   ```

12. Catalog error behavior:

   ```text
   generic "Pricing is temporarily unavailable."
   Retry action
   no hardcoded fallback
   no fake zero prices
   entitlement status loaded independently
   local and SSH features unaffected
   ```

13. Checkout flow:

   ```text
   selected opaque offer_id
   -> ditchd
   -> POST Relay offer checkout
   -> opaque hosted_url
   -> system browser
   -> Finishing upgrade...
   -> authoritative Relay entitlement polling
   ```

Browser return is never payment proof.

Preserve license redemption, Manage Billing, signed Commercial artifact authorization, verification, installation, state compatibility, and one-runtime behavior.

## Community Contract Artifacts and Drift Gate

Add or complete a JSON Schema for the canonical fixture under `docs/contracts/`. Validate the fixture against it in Community CI/tests.

Provide a deterministic export/checksum mechanism that a private Relay checkout can consume and verify using an explicitly pinned Community revision. Requirements:

```text
Community remains independently buildable
Community never depends on the private Relay
normal Flutter/Rust builds require no neighboring repository
contract artifact is versioned
export is deterministic
schema/fixture drift fails CI
```

Do not place private Relay source, provider IDs, Remote Control protocol, pairing, or mobile command implementation into Community.

## Tests

Add or strengthen tests for:

```text
canonical fixture deserializes and validates in Rust
protocol version mismatch fails
numeric refreshed_at fails with a useful invalid-response error
malformed RFC 3339 refreshed_at fails
standard Monthly parses and renders
paid introductory Monthly parses and renders with strikethrough
introductory amount must be below base amount
Lifetime parses and renders
eligible Lifetime Extra Pair appears as add capacity, not first purchase
multiple active offers render independently
locale/currency/exponent formatting
unknown currency/excessive exponent/malformed amount fails safely
opaque offer ID passes unchanged
checkout hosted URL opens
pending checkout waits for entitlement
failed/cancelled checkout never claims activation
active subscription hides acquisition cards
catalog failure shows generic Retry without price fallback
entitlement request can still succeed when catalog fails
local and SSH Community behavior remains available
```

Run at minimum from the repository root:

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
scripts/check-community-leakage
cd apps/macos
flutter analyze
flutter test
flutter build macos
```

If the environment prevents Xcode, Flutter, network, or signing steps, report the exact limitation. Do not claim a blocked command passed.

## Manual Acceptance After the Separate Relay Fix Is Deployed

Do not deploy the Relay from this task. Once its owner separately deploys the compatible Relay build, verify:

```text
1. Open Settings -> Remote Control.
2. Catalog cards replace "Pricing is temporarily unavailable."
3. Monthly standard price comes only from Relay/Stripe.
4. Eligible introductory price shows the base price struck through and the paid introductory duration.
5. Lifetime comes only from Relay/Stripe.
6. No provider branding or IDs appear.
7. Selecting an offer opens hosted Checkout.
8. Finishing upgrade waits for Relay entitlement.
9. Active subscribers no longer see ordinary acquisition packages.
10. Local and SSH Community functionality remains unchanged.
```

## Final Report

Report only Community/macOS changes:

1. files changed;
2. final Rust and Dart contract models;
3. validation added;
4. pricing and introductory UI behavior;
5. IPC and checkout behavior;
6. Community contract artifact/export gate;
7. tests run and exact results;
8. anything that remains blocked on the independent Relay implementation or deployment.

Do not claim the live issue is fixed merely because fixture tests pass. Live resolution requires a compatible Relay deployment and an authenticated end-to-end macOS request.
