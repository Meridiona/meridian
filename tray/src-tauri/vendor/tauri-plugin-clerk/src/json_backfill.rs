//ambient dev tool that watches what you do and updates your PM tickets automatically, boosting developer productivity
//! Backfills fields guest-js's Clerk-object serializers omit but
//! `clerk-fapi-rs`'s OpenAPI-generated models require present — see the
//! parent module doc (`lib.rs`) for the full incident writeup and
//! <https://github.com/Nipsuli/tauri-plugin-clerk/issues/7> for the upstream
//! report this mirrors.
//!
//! # Design: dispatch on the `"object"` discriminator, not a path list
//! Every Clerk-serialized JSON object carries an `"object"` field naming its
//! own type (`"email_address"`, `"user"`, `"session"`, …) — see
//! `clerkClientToClientJSON` and friends in the `tauri-plugin-clerk` npm
//! package's `guest-js/clerk-utils.ts`. [`backfill_clerk_client_json`] walks
//! the whole tree recursively and, at every object, dispatches on that
//! discriminator rather than a hardcoded field path (`client.sessions[].user…`).
//! That means it also covers `email_addresses`/`phone_numbers`/`web3_wallets`
//! nested inside `sign_in`/`sign_up` (not just the top-level user), and any
//! future nesting depth, without new cases — the same objects need the same
//! defaults wherever they appear.

use serde_json::{json, Map, Value};

/// Object-type-keyed field defaults, applied only when the key is ABSENT
/// (never overwrites a value guest-js did emit). Timestamp fields
/// (`created_at`/`updated_at`/`last_active_at`/`expire_at`/`abandon_at`) are
/// handled separately below since that's a float-to-int conversion on a
/// PRESENT value, not a missing-key backfill.
type FieldDefault = (&'static str, fn() -> Value);

fn defaults_for(object_type: &str) -> &'static [FieldDefault] {
    match object_type {
        // Table from upstream issue #7: these three object types all lack
        // `reserved`/`created_at`/`updated_at` in the guest-js payload.
        "email_address" | "phone_number" | "web3_wallet" => &[
            ("reserved", || json!(false)),
            ("created_at", || json!(0)),
            ("updated_at", || json!(0)),
        ],
        // Additionally lacks `verification` (the "latent" field issue #7
        // calls out for OAuth users specifically).
        "external_account" => &[
            ("reserved", || json!(false)),
            ("created_at", || json!(0)),
            ("updated_at", || json!(0)),
            ("verification", || Value::Null),
        ],
        "user" => &[
            ("saml_accounts", || json!([])),
            ("banned", || json!(false)),
            ("locked", || json!(false)),
            ("lockout_expires_in_seconds", || Value::Null),
            ("verification_attempts_remaining", || Value::Null),
            ("last_active_at", || Value::Null),
            ("mfa_enabled_at", || Value::Null),
            ("mfa_disabled_at", || Value::Null),
        ],
        // clerk-js gives `[number, number] | null`; a `null` would itself
        // fail to deserialize into `Vec<i64>` too, but guest-js's `?? []`
        // already handles that — this only covers the KEY being absent.
        "session" => &[("factor_verification_age", || json!([]))],
        _ => &[],
    }
}

/// Timestamp keys clerk-fapi-rs types as `i64`, which guest-js emits as
/// `date.getTime() / 1e3` — a fractional-second float (e.g.
/// `1719765690.123`) that `serde_json` will not coerce into an `i64`.
const TIMESTAMP_KEYS: &[&str] = &[
    "created_at",
    "updated_at",
    "last_active_at",
    "expire_at",
    "abandon_at",
    "last_sign_in_at",
    "password_last_updated_at",
    "legal_accepted_at",
    "cookie_expires_at",
    "mfa_enabled_at",
    "mfa_disabled_at",
    "last_used_at",
];

/// guest-js serializes the client's in-flight `sign_in`/`sign_up` resources
/// with the short JS-side object name (`"sign_in"` / `"sign_up"`) instead of
/// clerk-js's real `object` value. `clerk-fapi-rs`'s generated `ClientSignIn`/
/// `ClientSignUp` models each type `object` as a single-variant enum that only
/// accepts `"sign_in_attempt"` / `"sign_up_attempt"` (see
/// `client_sign_in.rs`/`client_sign_up.rs` upstream), so the mismatch fails
/// deserialization with `unknown variant "sign_in", expected "sign_in_attempt"`
/// on every real sign-in — a fresh sibling of the missing-field bug this module
/// otherwise fixes, but on a key that IS present rather than absent, so
/// `defaults_for`'s `entry(..).or_insert(..)` can never reach it; the value
/// has to be rewritten in place instead.
fn canonical_object_type(raw: &str) -> &str {
    match raw {
        "sign_in" => "sign_in_attempt",
        "sign_up" => "sign_up_attempt",
        other => other,
    }
}

/// Recursively normalize a Clerk `client` JSON tree (or any subtree of one)
/// so it deserializes cleanly into `clerk-fapi-rs`'s models. Mutates in
/// place; safe to call on an already-well-formed payload (every backfill is
/// an `entry(..).or_insert(..)` and every timestamp rewrite is a no-op on an
/// already-integral value, and `canonical_object_type` is a no-op on an
/// already-canonical `object`).
pub(crate) fn backfill_clerk_client_json(value: &mut Value) {
    match value {
        Value::Array(items) => {
            for item in items {
                backfill_clerk_client_json(item);
            }
        }
        Value::Object(map) => {
            floor_timestamp_fields(map);
            if let Some(object_type) = map.get("object").and_then(Value::as_str) {
                // Borrow-check: collect the (owned) defaults before touching
                // `map` again, since `object_type` borrows from it.
                let object_type = object_type.to_string();
                let canonical = canonical_object_type(&object_type).to_string();
                if canonical != object_type {
                    map.insert("object".to_string(), json!(canonical));
                }
                for (key, default) in defaults_for(&canonical) {
                    map.entry(*key).or_insert_with(default);
                }
            }
            for v in map.values_mut() {
                backfill_clerk_client_json(v);
            }
        }
        _ => {}
    }
}

fn floor_timestamp_fields(map: &mut Map<String, Value>) {
    for key in TIMESTAMP_KEYS {
        if let Some(f) = map.get(*key).and_then(Value::as_f64) {
            map.insert((*key).to_string(), json!(f.floor() as i64));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clerk_fapi_rs::models::ClientClient;

    /// A realistic client JSON payload for a signed-in email+password user
    /// belonging to an Organization, shaped exactly as guest-js's
    /// `clerkClientToClientJSON`/`clerkSessionToSessionJSON`/
    /// `clerkUserToUserJSON`/`clerkEmailAddressToEmailAdressJSON` actually
    /// emit it (per `tauri-plugin-clerk`'s `dist-js/index.js`) — i.e. WITH
    /// the fields upstream issue #7 documents as missing, and with
    /// fractional-second timestamps. This is the exact shape that, before
    /// this fix, made `set_client` silently never run for a real user.
    ///
    /// Deliberately omits: `sessions[].factor_verification_age` (clerk-js
    /// gave `null`, guest-js's `?? []` only fixes the Vec-vs-null shape, not
    /// a missing key), `sessions[].user.email_addresses[].{reserved,
    /// created_at,updated_at}`, and `sessions[].user.{saml_accounts,banned,
    /// locked,lockout_expires_in_seconds,verification_attempts_remaining,
    /// last_active_at,mfa_enabled_at,mfa_disabled_at}` — the exact set from
    /// issue #7's table. Parsed from a raw string (not the `json!` macro) to
    /// keep this fixture's macro-expansion depth off the crate's recursion
    /// limit.
    fn guest_js_shaped_client_json() -> Value {
        serde_json::from_str(
            r#"{
                "object": "client",
                "id": "client_abc123",
                "sessions": [{
                    "object": "session",
                    "id": "sess_abc123",
                    "status": "active",
                    "expire_at": 1719765690.123,
                    "abandon_at": 1719765690.123,
                    "last_active_at": 1719765690.123,
                    "last_active_token": { "object": "token", "id": "tok_1", "jwt": "jwt-value" },
                    "last_active_organization_id": null,
                    "actor": null,
                    "tasks": null,
                    "user": {
                        "object": "user",
                        "id": "user_abc123",
                        "external_id": null,
                        "primary_email_address_id": "idn_email1",
                        "primary_phone_number_id": null,
                        "primary_web3_wallet_id": null,
                        "image_url": "https://img.clerk.com/x",
                        "has_image": true,
                        "username": null,
                        "email_addresses": [{
                            "object": "email_address",
                            "id": "idn_email1",
                            "email_address": "user@example.com",
                            "linked_to": [],
                            "matches_sso_connection": false,
                            "verification": null
                        }],
                        "phone_numbers": [],
                        "web3_wallets": [],
                        "external_accounts": [],
                        "enterprise_accounts": [],
                        "passkeys": [],
                        "organization_memberships": [],
                        "password_enabled": true,
                        "profile_image_id": "https://img.clerk.com/x",
                        "first_name": "Test",
                        "last_name": "User",
                        "totp_enabled": false,
                        "backup_code_enabled": false,
                        "two_factor_enabled": false,
                        "public_metadata": {},
                        "unsafe_metadata": {},
                        "last_sign_in_at": 1719765690.123,
                        "create_organization_enabled": true,
                        "create_organizations_limit": null,
                        "delete_self_enabled": true,
                        "legal_accepted_at": null,
                        "updated_at": 1719765690.123,
                        "created_at": 1719765690.123
                    },
                    "public_user_data": null,
                    "created_at": 1719765690.123,
                    "updated_at": 1719765690.123
                }],
                "sign_up": null,
                "sign_in": null,
                "captcha_bypass": false,
                "last_active_session_id": "sess_abc123",
                "cookie_expires_at": null,
                "created_at": 1719765600.0,
                "updated_at": 1719765690.123
            }"#,
        )
        .expect("fixture literal must be valid JSON")
    }

    #[test]
    fn without_backfill_the_guest_js_shape_fails_to_deserialize() {
        // Pins the regression this whole module exists to fix: without the
        // backfill, this payload — representative of ANY real signed-in
        // user — does not parse into clerk-fapi-rs's model.
        let value = guest_js_shaped_client_json();
        assert!(
            serde_json::from_value::<ClientClient>(value).is_err(),
            "fixture no longer reproduces the upstream bug — if clerk-fapi-rs \
             relaxed its schema, this test (not the fix) should be revisited"
        );
    }

    #[test]
    fn backfill_makes_the_guest_js_shape_deserialize() {
        let mut value = guest_js_shaped_client_json();
        backfill_clerk_client_json(&mut value);
        let client = serde_json::from_value::<ClientClient>(value)
            .expect("backfilled payload must deserialize into clerk-fapi-rs's ClientClient");
        assert_eq!(client.sessions.len(), 1);
        let session = &client.sessions[0];
        assert!(session.factor_verification_age.is_empty());
        let user = session.user.as_ref().expect("session must carry its user");
        assert!(!user.banned);
        assert!(!user.locked);
        assert!(user.saml_accounts.is_empty());
        assert_eq!(user.email_addresses.len(), 1);
        assert!(!user.email_addresses[0].reserved);
        // The fractional timestamps must have survived as the same second,
        // just floored to an integer — not zeroed out.
        assert_eq!(client.updated_at, 1719765690);
    }

    #[test]
    fn backfill_never_overwrites_a_value_guest_js_did_emit() {
        let mut value = json!({
            "object": "user",
            "banned": true,
            "saml_accounts": [{"object": "saml_account", "id": "saml_1"}]
        });
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["banned"], json!(true));
        assert_eq!(value["saml_accounts"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn backfill_is_idempotent_on_an_already_well_formed_payload() {
        let mut value = guest_js_shaped_client_json();
        backfill_clerk_client_json(&mut value);
        let once = value.clone();
        backfill_clerk_client_json(&mut value);
        assert_eq!(once, value);
    }

    #[test]
    fn floors_fractional_timestamps_at_every_depth() {
        let mut value = json!({
            "object": "client",
            "created_at": 100.999,
            "sessions": [{ "object": "session", "created_at": 200.001 }]
        });
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["created_at"], json!(100));
        assert_eq!(value["sessions"][0]["created_at"], json!(200));
    }

    /// A client's in-flight sign-in, shaped exactly as guest-js emits it: the
    /// `object` discriminator is the short JS resource name `"sign_in"`, not
    /// clerk-fapi-rs's `"sign_in_attempt"`. Every other field is present and
    /// well-typed — this is purely the discriminator mismatch production hit.
    fn guest_js_shaped_sign_in_json() -> Value {
        json!({
            "object": "sign_in",
            "id": "sign_in_abc123",
            "status": "needs_first_factor",
            "supported_identifiers": ["email_address"],
            "supported_first_factors": null,
            "supported_second_factors": null,
            "first_factor_verification": null,
            "second_factor_verification": null,
            "identifier": "user@example.com",
            "user_data": null,
            "created_session_id": null,
            "abandon_at": 1719765690
        })
    }

    /// Pins the production bug: `unknown variant "sign_in", expected
    /// "sign_in_attempt"`, hit by every real user with an in-flight sign-in.
    #[test]
    fn without_the_fix_guest_js_sign_in_object_type_fails_to_deserialize() {
        let value = guest_js_shaped_sign_in_json();
        assert!(
            serde_json::from_value::<clerk_fapi_rs::models::ClientSignIn>(value).is_err(),
            "fixture no longer reproduces the upstream mismatch — if clerk-fapi-rs \
             relaxed its schema, this test (not the fix) should be revisited"
        );
    }

    #[test]
    fn backfill_canonicalizes_the_sign_in_object_type() {
        let mut value = guest_js_shaped_sign_in_json();
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["object"], json!("sign_in_attempt"));
        serde_json::from_value::<clerk_fapi_rs::models::ClientSignIn>(value).expect(
            "backfilled sign_in payload must deserialize into clerk-fapi-rs's ClientSignIn",
        );
    }

    /// The same rewrite applied recursively, inside a whole client tree —
    /// not just when `ClientSignIn` is deserialized standalone.
    #[test]
    fn backfill_canonicalizes_sign_in_nested_inside_a_client() {
        let mut value = json!({
            "object": "client",
            "sign_in": guest_js_shaped_sign_in_json(),
            "sign_up": null,
        });
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["sign_in"]["object"], json!("sign_in_attempt"));
    }

    /// `sign_up` carries the identical bug class (`"sign_up"` vs
    /// `"sign_up_attempt"`) — covered at the canonicalizer level since a full
    /// `ClientSignUp` fixture needs a `verifications` sub-object this bug has
    /// nothing to do with.
    #[test]
    fn canonicalizes_sign_up_the_same_way_as_sign_in() {
        assert_eq!(canonical_object_type("sign_up"), "sign_up_attempt");
    }

    /// An already-canonical `object` must not be touched — only the two known
    /// guest-js short names are remapped.
    #[test]
    fn canonical_object_type_is_a_no_op_on_correct_and_unrelated_values() {
        assert_eq!(canonical_object_type("sign_in_attempt"), "sign_in_attempt");
        assert_eq!(canonical_object_type("user"), "user");
    }

    /// A client's in-flight sign-up, shaped as guest-js emits it — the
    /// `sign_up` counterpart to `guest_js_shaped_sign_in_json`. `verifications`
    /// is the one field `ClientSignUp` requires that `ClientSignIn` does not
    /// (each of its four sub-fields is `Option::deserialize`, so `null` is
    /// valid but the KEY must be present).
    fn guest_js_shaped_sign_up_json() -> Value {
        json!({
            "object": "sign_up",
            "id": "sign_up_abc123",
            "status": "missing_requirements",
            "required_fields": ["email_address"],
            "optional_fields": [],
            "missing_fields": ["email_address"],
            "unverified_fields": [],
            "verifications": {
                "email_address": null,
                "phone_number": null,
                "web3_wallet": null,
                "external_account": null
            },
            "username": null,
            "email_address": "user@example.com",
            "phone_number": null,
            "web3_wallet": null,
            "password_enabled": true,
            "first_name": null,
            "last_name": null,
            "custom_action": false,
            "external_id": null,
            "created_session_id": null,
            "created_user_id": null,
            "abandon_at": 1719765690,
            "legal_accepted_at": null
        })
    }

    /// The `sign_up` counterpart to `without_the_fix_guest_js_sign_in_object_type_fails_to_deserialize`
    /// — pins that the discriminator mismatch is not sign_in-specific.
    #[test]
    fn without_the_fix_guest_js_sign_up_object_type_fails_to_deserialize() {
        let value = guest_js_shaped_sign_up_json();
        assert!(
            serde_json::from_value::<clerk_fapi_rs::models::ClientSignUp>(value).is_err(),
            "fixture no longer reproduces the upstream mismatch — if clerk-fapi-rs \
             relaxed its schema, this test (not the fix) should be revisited"
        );
    }

    #[test]
    fn backfill_canonicalizes_the_sign_up_object_type() {
        let mut value = guest_js_shaped_sign_up_json();
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["object"], json!("sign_up_attempt"));
        serde_json::from_value::<clerk_fapi_rs::models::ClientSignUp>(value).expect(
            "backfilled sign_up payload must deserialize into clerk-fapi-rs's ClientSignUp",
        );
    }

    #[test]
    fn backfill_canonicalizes_sign_up_nested_inside_a_client() {
        let mut value = json!({
            "object": "client",
            "sign_in": null,
            "sign_up": guest_js_shaped_sign_up_json(),
        });
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["sign_up"]["object"], json!("sign_up_attempt"));
    }

    /// Idempotency across the REWRITE path specifically — the existing
    /// `backfill_is_idempotent_on_an_already_well_formed_payload` only covers
    /// the original fixture, whose `sign_in`/`sign_up` are `null` and so never
    /// touch `canonical_object_type` at all.
    #[test]
    fn backfill_is_idempotent_after_canonicalizing_a_populated_sign_in() {
        let mut value = json!({
            "object": "client",
            "sign_in": guest_js_shaped_sign_in_json(),
            "sign_up": null,
        });
        backfill_clerk_client_json(&mut value);
        let once = value.clone();
        backfill_clerk_client_json(&mut value);
        assert_eq!(
            once, value,
            "a second pass over an already-canonicalized sign_in must be a no-op"
        );
    }

    /// The full end-to-end case none of the standalone `ClientSignIn` tests
    /// reach: a whole `ClientClient` with a LIVE (non-null) `sign_in`, shaped
    /// exactly as guest-js emits it end to end, deserializing successfully
    /// after backfill — not just the `object` field in isolation. This is the
    /// actual shape `set_client` receives in production the moment a user is
    /// mid-sign-in (MFA prompt, etc.) rather than already fully signed in.
    #[test]
    fn backfill_makes_a_client_with_a_live_sign_in_deserialize() {
        let mut value = json!({
            "object": "client",
            "id": "client_abc123",
            "sessions": [],
            "sign_in": guest_js_shaped_sign_in_json(),
            "sign_up": null,
            "last_active_session_id": null,
            "cookie_expires_at": null,
            "captcha_bypass": false,
            "created_at": 1719765600,
            "updated_at": 1719765690
        });
        backfill_clerk_client_json(&mut value);
        let client = serde_json::from_value::<ClientClient>(value).expect(
            "a client with a live sign_in must deserialize into ClientClient after backfill",
        );
        let sign_in = client.sign_in.expect("sign_in must survive the round trip");
        assert_eq!(sign_in.id, "sign_in_abc123");
    }
}
