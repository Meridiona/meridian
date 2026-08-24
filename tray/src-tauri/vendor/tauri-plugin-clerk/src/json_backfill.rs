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
        // `clerkSignInToSignInJSON` emits no `abandon_at` at all, but
        // `ClientSignIn.abandon_at` is a bare `i64`. Found by writing a
        // fixture straight off guest-js's real serializer rather than by
        // hand — the pre-existing `sign_in` fixture in this module's tests
        // included `abandon_at`, which guest-js never sends, so it round-tripped
        // and hid this. `0` matches how every other missing timestamp is
        // defaulted here.
        //
        // EVERY OTHER FIELD ON THE MODEL IS LISTED TOO, and that is the point —
        // see the `sign_up_attempt` note below for why this object type is
        // defaulted exhaustively rather than field-by-field as each one is
        // observed failing.
        "sign_in_attempt" => &[
            ("id", || json!("")),
            ("status", || json!("abandoned")),
            ("supported_identifiers", || json!([])),
            ("supported_first_factors", || Value::Null),
            ("supported_second_factors", || Value::Null),
            ("first_factor_verification", || Value::Null),
            ("second_factor_verification", || Value::Null),
            ("identifier", || Value::Null),
            ("user_data", || Value::Null),
            ("created_session_id", || Value::Null),
            ("abandon_at", || json!(0)),
        ],
        // `clerkSignUpToSignUpJSON` is missing more than one required field:
        // it emits `has_password` where the model wants `password_enabled`,
        // and never emits `custom_action` — both bare `bool`s upstream. It
        // DOES emit `abandon_at`, so that entry is a no-op on a real payload
        // and only guards a partial object.
        //
        // None of this had been observed in production yet. It is fixed anyway:
        // it is the same untouched guest-js code path that has now produced
        // three separate production incidents, and the cost of a fourth is the
        // whole offline session cache, silently.
        // `external_id` is `Option<String>` but carries
        // `deserialize_with = "Option::deserialize"`, which requires the KEY to
        // be present even for a null — and guest-js omits it entirely. That
        // distinction (optional VALUE, mandatory KEY) is the trap running
        // through this whole module: `Option<T>` reads as "safe to leave out"
        // and is not.
        //
        // # Why both scratch types are now defaulted EXHAUSTIVELY
        //
        // `client.sign_in` / `client.sign_up` are scratch objects: clerk-js
        // keeps them on every client whether or not an attempt is in flight,
        // and when none is, their fields are `undefined`. `JSON.stringify`
        // DROPS undefined-valued keys, so what reaches Rust for the ordinary
        // signed-in user is close to `{"object": "sign_up", "status": null}` —
        // nearly every required key absent at once.
        //
        // That is why this object type has now produced FOUR production
        // incidents in a row (`status`, `verifications`,
        // `password_enabled`/`custom_action`, and `id` — observed 2026-08-24,
        // firing every ~43 s). Each fix repaired the one field serde happened
        // to report, deserialization then advanced by one field, and the next
        // release failed on the next one. Listing every field the model
        // requires ends that sequence: there is no "next field" left.
        //
        // Fixtures could not catch this because a transcription of the
        // serializer is faithful to the CODE and silently wrong about the
        // RUNTIME - it shows `"id": signUp.id`, and a human writing the fixture
        // fills in `"sua_abc123"`. `a_bare_sign_up_scratch_object_deserializes_
        // after_backfill` tests the empty object instead, so it cannot rot the
        // same way.
        //
        // Every value here matches the model's own `impl Default` (`""` for the
        // `String` id, `abandoned` for the status enum, `[]` for the `Vec`s),
        // and each is an `or_insert`, so a key guest-js DID send always wins.
        "sign_up_attempt" => &[
            ("id", || json!("")),
            ("status", || json!("abandoned")),
            ("required_fields", || json!([])),
            ("optional_fields", || json!([])),
            ("missing_fields", || json!([])),
            ("unverified_fields", || json!([])),
            ("username", || Value::Null),
            ("email_address", || Value::Null),
            ("phone_number", || Value::Null),
            ("web3_wallet", || Value::Null),
            ("first_name", || Value::Null),
            ("last_name", || Value::Null),
            ("created_session_id", || Value::Null),
            ("created_user_id", || Value::Null),
            ("legal_accepted_at", || Value::Null),
            ("abandon_at", || json!(0)),
            ("password_enabled", || json!(false)),
            ("custom_action", || json!(false)),
            ("external_id", || Value::Null),
        ],
        _ => &[],
    }
}

/// Carry guest-js's `has_password` across to the `password_enabled` the model
/// wants, when the payload said so.
///
/// The rename is already documented in [`defaults_for`] — but the default
/// there is a flat `false`, which is only right when guest-js said nothing.
/// A sign-up that DID set a password sends `"has_password": true`, and
/// defaulting past it writes the opposite of the truth into the cached session
/// before `ClientSignUp` ever deserializes. Silent, and exactly wrong on the
/// account state a user would notice.
///
/// Runs BEFORE `defaults_for`'s `or_insert`, so the carried value wins and the
/// `false` default only applies when neither key holds a usable answer.
///
/// # A present key with the wrong TYPE is treated as absent
/// `ClientSignUp.password_enabled` is a bare `bool`, so `null`, a string, or a
/// number there fails deserialization exactly as a missing key does — and
/// `defaults_for`'s `or_insert` cannot rescue it, because the key exists. This
/// is the module's "optional VALUE, mandatory KEY" trap running the other way:
/// present, but unusable. Keying the guard on `contains_key` would leave that
/// payload broken, so it keys on actually holding a boolean.
fn carry_over_has_password(canonical: &str, map: &mut serde_json::Map<String, Value>) {
    if canonical != "sign_up_attempt" {
        return;
    }
    if map.get("password_enabled").is_some_and(Value::is_boolean) {
        return; // Already a usable answer — authoritative, never overwritten.
    }
    let carried = map
        .get("has_password")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    // Observability: the SHAPE, never the payload. `carried` is one bit of
    // account state and `had_key` says only whether a key was present — no
    // email, no token, no field values. Worth recording because this is a
    // silent rewrite of cached auth state, and because a `present_but_unusable`
    // hit means guest-js sent a type we did not expect, which is the leading
    // indicator for the next incident in this module's history.
    let present_but_unusable = map.contains_key("password_enabled");
    if present_but_unusable {
        tracing::warn!(
            object = "sign_up_attempt",
            carried,
            "clerk backfill: password_enabled present but not a boolean - normalized"
        );
    } else {
        tracing::debug!(
            object = "sign_up_attempt",
            carried,
            had_has_password = map.contains_key("has_password"),
            "clerk backfill: password_enabled filled in"
        );
    }
    map.insert("password_enabled".to_string(), json!(carried));
}

/// Ensure `verifications` is an object carrying ALL four keys.
///
/// Replacing only a non-object (the shape this started as) is not enough: every
/// field is `Option` but carries `deserialize_with = "Option::deserialize"`, so
/// a PARTIAL object — say one that names `email_address` alone — is missing
/// required keys just as surely as `{}` is, and fails with `missing field`
/// all the same. See [`empty_sign_up_verifications`] for the same reasoning
/// applied to the empty case.
///
/// Populated entries are preserved; only the absent keys get a `null`.
fn complete_sign_up_verifications(map: &mut serde_json::Map<String, Value>) {
    let mut filled = empty_sign_up_verifications();
    let mut carried_over = 0usize;
    if let (Value::Object(required), Some(Value::Object(existing))) =
        (&mut filled, map.get("verifications"))
    {
        // Start from the all-null skeleton and overlay whatever was populated,
        // so a key guest-js omitted is present-and-null rather than missing.
        for (key, value) in existing {
            required.insert(key.clone(), value.clone());
            carried_over += 1;
        }
    }
    // COUNTS ONLY. A verification entry carries the email address or phone
    // number being verified, so the keys and values stay out of the record
    // entirely - `carried_over` and `filled_in` are enough to tell "guest-js
    // sent a partial object" from "it sent nothing", which is the whole
    // diagnostic question here.
    let total = filled.as_object().map_or(0, serde_json::Map::len);
    tracing::debug!(
        object = "sign_up_attempt",
        carried_over,
        filled_in = total.saturating_sub(carried_over),
        "clerk backfill: sign_up verifications completed"
    );
    map.insert("verifications".to_string(), filled);
}

/// `clerkSignUpToSignUpJSON` hardcodes `verifications: null`, but
/// `ClientSignUp.verifications` is a non-`Option` `Box<ClientSignUpVerifications>`
/// — so the null is rejected outright.
///
/// The replacement has to spell out all four keys rather than being `{}`:
/// every field on `ClientSignUpVerifications` is `Option` but carries
/// `deserialize_with = "Option::deserialize"`, which requires the key to be
/// PRESENT even when its value is null. An empty object fails with
/// `missing field`.
fn empty_sign_up_verifications() -> Value {
    json!({
        "email_address": null,
        "phone_number": null,
        "web3_wallet": null,
        "external_account": null,
    })
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

/// The `status` fallback for an object type whose `status` guest-js can emit
/// as `null` while `clerk-fapi-rs` types it as a required, non-`Option` enum.
///
/// # Why this is a table rather than one special case
/// guest-js passes `status` straight through on `session`, `sign_in` and
/// `sign_up` alike (`status: session.status`, `status: signIn.status`,
/// `status: signUp.status` in `clerk-utils.ts`) with none of the `?? …`
/// fallbacks it applies to its other fields. Every one of those three lands in
/// a generated model whose `status` is a bare enum, so a `null` fails with
/// `invalid type: null, expected string or map` and takes the whole
/// `set_client` write down with it — no offline session cache, sign-in screen
/// on the next cold start.
///
/// This has now happened three times on three different fields:
/// `ClientSession.status` (fixed first), and `ClientSignIn.status` — observed
/// in production on 2026-08-20, firing twice every ~44 s on a machine whose
/// user WAS signed in, which is why the original fix appeared to work while
/// persistence stayed broken. `sign_up` had not been seen yet and is included
/// anyway: it is the same field, emitted by the same untouched code path, into
/// the same shape of model. Fixing only what has been observed is what turned
/// one bug into three.
///
/// Each fallback matches that model's own `impl Default for Status`, so the
/// value chosen here is the one the library itself would pick:
/// `session` → `active` (`client_session::Status`), `sign_in_attempt` →
/// `abandoned` (`client_sign_in::Status`), `sign_up_attempt` → `abandoned`
/// (`client_sign_up::Status`). `abandoned` is also the semantically right
/// answer: a client's `sign_in`/`sign_up` are scratch objects for an
/// in-flight attempt, and a null status means there is no attempt in flight.
fn null_status_fallback(canonical_object_type: &str) -> Option<&'static str> {
    match canonical_object_type {
        "session" => Some("active"),
        "sign_in_attempt" | "sign_up_attempt" => Some("abandoned"),
        _ => None,
    }
}

/// True when `status` is present but `null` — the case
/// [`defaults_for`]'s `entry(..).or_insert(..)` structurally cannot reach,
/// since the key exists.
fn has_null_status(map: &Map<String, Value>) -> bool {
    matches!(map.get("status"), Some(Value::Null))
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
                // BEFORE the blanket defaults: `password_enabled` has a real
                // source in the payload and must not be defaulted past it.
                carry_over_has_password(&canonical, map);
                for (key, default) in defaults_for(&canonical) {
                    map.entry(*key).or_insert_with(default);
                }
                if has_null_status(map) {
                    if let Some(fallback) = null_status_fallback(&canonical) {
                        map.insert("status".to_string(), json!(fallback));
                    }
                }
                if canonical == "sign_up_attempt" {
                    complete_sign_up_verifications(map);
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

    /// An in-flight sign-up with the `object` discriminator guest-js uses but
    /// otherwise ALREADY-VALID field values — used only by the discriminator
    /// tests below, which is all it is good for.
    ///
    /// It is deliberately NOT what guest-js emits, despite what its name
    /// suggests, and the mismatch is worth reading before trusting any fixture
    /// in this module: a hand-written "realistic" payload quietly supplied
    /// `password_enabled`, `custom_action`, a populated `verifications` and an
    /// integral `abandon_at` — all four of which guest-js gets wrong or omits.
    /// Because those tests passed, three real defects on this object went
    /// unnoticed until `real_guest_js_sign_up_json` was transcribed
    /// field-for-field from the npm package's `dist-js/index.js`. When adding a
    /// fixture here, copy the serializer, do not describe it.
    fn idealized_sign_up_json() -> Value {
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
        let value = idealized_sign_up_json();
        assert!(
            serde_json::from_value::<clerk_fapi_rs::models::ClientSignUp>(value).is_err(),
            "fixture no longer reproduces the upstream mismatch — if clerk-fapi-rs \
             relaxed its schema, this test (not the fix) should be revisited"
        );
    }

    #[test]
    fn backfill_canonicalizes_the_sign_up_object_type() {
        let mut value = idealized_sign_up_json();
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
            "sign_up": idealized_sign_up_json(),
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

    /// A `session` object shaped exactly as `clerkSessionToSessionJSON` emits
    /// it when `Session.status` is `null` — the same realistic shape as
    /// `guest_js_shaped_client_json`'s nested session, with only `status`
    /// changed. Observed live in production: this is what made offline
    /// sign-in caching silently stop working even after the `sign_in`/
    /// `sign_up` discriminator fix.
    fn guest_js_shaped_session_with_null_status_json() -> Value {
        serde_json::from_str(
            r#"{
                "object": "session",
                "id": "sess_abc123",
                "status": null,
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
            }"#,
        )
        .expect("fixture literal must be valid JSON")
    }

    /// Pins the production bug: `invalid type: null, expected string or map`,
    /// hit whenever clerk-js hands guest-js a session whose `status` hasn't
    /// been set yet.
    #[test]
    fn without_the_fix_a_null_session_status_fails_to_deserialize() {
        let value = guest_js_shaped_session_with_null_status_json();
        assert!(
            serde_json::from_value::<clerk_fapi_rs::models::ClientSession>(value).is_err(),
            "fixture no longer reproduces the production null-status bug — if clerk-fapi-rs \
             relaxed its schema, this test (not the fix) should be revisited"
        );
    }

    #[test]
    fn backfill_falls_back_a_null_session_status_to_active() {
        let mut value = guest_js_shaped_session_with_null_status_json();
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["status"], json!("active"));
        serde_json::from_value::<clerk_fapi_rs::models::ClientSession>(value)
            .expect("a session backfilled from a null status must deserialize into ClientSession");
    }

    #[test]
    fn backfill_never_overwrites_a_non_null_session_status() {
        let mut value = json!({ "object": "session", "status": "ended" });
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["status"], json!("ended"));
    }

    /// The same rewrite applied recursively, inside a whole client tree —
    /// not just when `ClientSession` is deserialized standalone.
    #[test]
    fn backfill_falls_back_a_null_session_status_nested_inside_a_client() {
        let mut value = json!({
            "object": "client",
            "sessions": [guest_js_shaped_session_with_null_status_json()],
        });
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["sessions"][0]["status"], json!("active"));
    }

    #[test]
    fn backfill_is_idempotent_after_falling_back_a_null_session_status() {
        let mut value = guest_js_shaped_session_with_null_status_json();
        backfill_clerk_client_json(&mut value);
        let once = value.clone();
        backfill_clerk_client_json(&mut value);
        assert_eq!(
            once, value,
            "a second pass over an already-fallen-back session must be a no-op"
        );
    }

    /// A client's `sign_in` scratch object as `clerkSignInToSignInJSON`
    /// actually emits it when no sign-in attempt is in flight: `status` passed
    /// straight through as `null`.
    ///
    /// This is the SECOND null-status field to reach production. On
    /// 2026-08-20 a machine whose user was genuinely signed in logged
    /// `invalid type: null, expected string or map` at
    /// `payload.client.sign_in.status` twice every ~44 s — so `set_client`
    /// never ran, the offline cache stayed empty, and the previous
    /// null-session-status fix looked like it had worked while session
    /// persistence was still completely broken.
    fn guest_js_shaped_sign_in_with_null_status_json() -> Value {
        serde_json::from_str(
            r#"{
                "object": "sign_in",
                "id": "sia_abc123",
                "status": null,
                "supported_identifiers": [],
                "identifier": null,
                "user_data": {
                    "first_name": "",
                    "last_name": "",
                    "image_url": "",
                    "has_image": false
                },
                "supported_first_factors": [],
                "supported_second_factors": [],
                "first_factor_verification": null,
                "second_factor_verification": null,
                "created_session_id": null
            }"#,
        )
        .expect("fixture literal must be valid JSON")
    }

    /// Pins the production failure before asserting the fix, so a
    /// clerk-fapi-rs schema relaxation shows up here as a stale test rather
    /// than as a fix that silently stopped being needed.
    #[test]
    fn without_the_fix_a_null_sign_in_status_fails_to_deserialize() {
        let mut value = guest_js_shaped_sign_in_with_null_status_json();
        // Canonicalize `object` only, so this isolates the STATUS failure
        // rather than re-testing the discriminator bug.
        value["object"] = json!("sign_in_attempt");
        assert!(
            serde_json::from_value::<clerk_fapi_rs::models::ClientSignIn>(value).is_err(),
            "fixture no longer reproduces the production null-sign_in-status bug"
        );
    }

    #[test]
    fn backfill_falls_back_a_null_sign_in_status_to_abandoned() {
        let mut value = guest_js_shaped_sign_in_with_null_status_json();
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["status"], json!("abandoned"));
        serde_json::from_value::<clerk_fapi_rs::models::ClientSignIn>(value)
            .expect("a sign_in backfilled from a null status must deserialize into ClientSignIn");
    }

    /// The shape actually observed in production: the null status is nested on
    /// the CLIENT, alongside a real signed-in session. Deserializing the whole
    /// `ClientClient` is the operation that was failing, and it is the only one
    /// that matters - a standalone `ClientSignIn` is never parsed at runtime.
    #[test]
    fn a_client_carrying_a_null_status_sign_in_deserializes_after_backfill() {
        let mut value = guest_js_shaped_client_json();
        value["sign_in"] = guest_js_shaped_sign_in_with_null_status_json();
        assert!(
            serde_json::from_value::<ClientClient>(value.clone()).is_err(),
            "fixture must reproduce the production failure before the backfill runs"
        );
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["sign_in"]["status"], json!("abandoned"));
        serde_json::from_value::<ClientClient>(value)
            .expect("this is the exact payload that broke session persistence in production");
    }

    /// `sign_up` is the same field on the same untouched guest-js code path
    /// into the same shape of model, so it is covered pre-emptively. Fixing
    /// only the field that had been observed is what turned one bug into three.
    #[test]
    fn backfill_falls_back_a_null_sign_up_status_to_abandoned() {
        let mut value = json!({ "object": "sign_up", "status": null });
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["status"], json!("abandoned"));
    }

    /// A client's `sign_up` scratch object exactly as
    /// `clerkSignUpToSignUpJSON` emits it — transcribed field-for-field from
    /// the npm package's `dist-js/index.js`, deliberately NOT hand-tidied.
    /// Writing the `sign_in` fixture this way is what exposed the missing
    /// `abandon_at`; the pre-existing hand-written one had silently included
    /// fields guest-js never sends.
    ///
    /// Note what it does and does not contain: `has_password` (the model wants
    /// `password_enabled`), no `custom_action` at all, and
    /// `verifications: null` against a non-`Option` field.
    fn real_guest_js_sign_up_json() -> Value {
        serde_json::from_str(
            r#"{
                "object": "sign_up",
                "id": "sua_abc123",
                "status": null,
                "required_fields": ["email_address"],
                "optional_fields": [],
                "missing_fields": [],
                "unverified_fields": [],
                "username": null,
                "first_name": null,
                "last_name": null,
                "email_address": null,
                "phone_number": null,
                "web3_wallet": null,
                "external_account_strategy": null,
                "external_account": null,
                "has_password": false,
                "unsafe_metadata": {},
                "created_session_id": null,
                "created_user_id": null,
                "abandon_at": 1719765690.123,
                "legal_accepted_at": null,
                "verifications": null,
                "locale": null
            }"#,
        )
        .expect("fixture literal must be valid JSON")
    }

    /// Three separate defects on one object - a null `status`, a null
    /// `verifications` against a non-Option field, and two required bools
    /// guest-js never emits. None had reached production; all three would have
    /// taken down the same `set_client` write the moment a client carried a
    /// sign-up attempt.
    #[test]
    fn a_client_carrying_a_guest_js_shaped_sign_up_deserializes_after_backfill() {
        let mut value = guest_js_shaped_client_json();
        value["sign_up"] = real_guest_js_sign_up_json();
        assert!(
            serde_json::from_value::<ClientClient>(value.clone()).is_err(),
            "fixture must reproduce the failure before the backfill runs"
        );
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["sign_up"]["status"], json!("abandoned"));
        assert_eq!(value["sign_up"]["password_enabled"], json!(false));
        assert_eq!(value["sign_up"]["custom_action"], json!(false));
        assert!(value["sign_up"]["verifications"].is_object());
        serde_json::from_value::<ClientClient>(value)
            .expect("a guest-js-shaped sign_up must survive the round trip");
    }

    /// Pins the production failure observed 2026-08-24 on 1.90.0-staging.5:
    /// `missing field `id``, firing every ~43 s on a machine whose user WAS
    /// signed in.
    ///
    /// # Why every fixture in this module hid it
    /// `client.signIn` / `client.signUp` are SCRATCH objects — they exist even
    /// when no attempt is in flight, and then their `id` is `undefined`.
    /// `JSON.stringify` drops undefined-valued keys entirely, so the key never
    /// reaches the wire. Transcribing the serializer (as
    /// [`real_guest_js_sign_up_json`] correctly did) still gives you `"id":
    /// signUp.id` and a plausible-looking `"sua_abc123"` filled in by hand —
    /// the transcription is faithful to the CODE and wrong about the RUNTIME.
    /// That is the same trap the `abandon_at` note in [`idealized_sign_up_json`]
    /// describes, one level deeper: it is not enough to copy the serializer, the
    /// fixture also has to reflect what its inputs actually hold.
    /// `id` is dropped AFTER the backfill runs, so every other defect on this
    /// fixture (the null `status`, the null `verifications`, the float
    /// `abandon_at`, the renamed `has_password`) is already repaired and the
    /// absent `id` is provably the sole remaining cause. Removing it from the
    /// raw fixture instead would report whichever defect serde happened to
    /// reach first — which is the float `abandon_at`, not `id`.
    #[test]
    fn a_sign_up_whose_only_remaining_defect_is_a_missing_id_names_that_field() {
        let mut value = real_guest_js_sign_up_json();
        backfill_clerk_client_json(&mut value);
        value
            .as_object_mut()
            .expect("fixture is an object")
            .remove("id");
        let err = serde_json::from_value::<clerk_fapi_rs::models::ClientSignUp>(value)
            .expect_err("a sign_up with no id must not deserialize");
        assert!(
            err.to_string().contains("missing field `id`"),
            "expected the exact error production reported, got: {err}"
        );
    }

    /// The completeness proof for `sign_in`, and the reason it is shaped this
    /// way rather than as another transcribed fixture.
    ///
    /// The input is the worst case the wire can carry: the discriminator and
    /// NOTHING else. A test built on a realistic payload can only ever prove
    /// that payload works, which is precisely how four separate defects
    /// (`status`, `verifications`, `password_enabled`/`custom_action`, `id`)
    /// went to production on this one object. This cannot pass while any
    /// required key is unhandled, and it keeps holding if clerk-fapi-rs adds
    /// one — a new required field breaks it immediately rather than six weeks
    /// later on a user's machine.
    #[test]
    fn a_bare_sign_in_scratch_object_deserializes_after_backfill() {
        let mut value = json!({ "object": "sign_in" });
        backfill_clerk_client_json(&mut value);
        serde_json::from_value::<clerk_fapi_rs::models::ClientSignIn>(value).expect(
            "a sign_in carrying only its discriminator must round-trip - guest-js omits \
             every key whose value is undefined, which is all of them when no sign-in is \
             in flight",
        );
    }

    /// The `sign_up` half of
    /// [`a_bare_sign_in_scratch_object_deserializes_after_backfill`].
    #[test]
    fn a_bare_sign_up_scratch_object_deserializes_after_backfill() {
        let mut value = json!({ "object": "sign_up" });
        backfill_clerk_client_json(&mut value);
        serde_json::from_value::<clerk_fapi_rs::models::ClientSignUp>(value).expect(
            "a sign_up carrying only its discriminator must round-trip - see the sign_in \
             counterpart for why this is the shape that proves completeness",
        );
    }

    /// The end-to-end case, which is the only one that reflects runtime: a
    /// standalone `ClientSignIn` is never parsed on its own — `set_client`
    /// deserializes the whole `ClientClient` tree, so one unhandled key inside
    /// a scratch object takes the entire session write down with it.
    #[test]
    fn a_client_whose_scratch_objects_have_no_id_survives_the_round_trip() {
        let mut value = guest_js_shaped_client_json();
        value["sign_in"] = json!({ "object": "sign_in", "status": null });
        value["sign_up"] = json!({ "object": "sign_up", "status": null });
        assert!(
            serde_json::from_value::<ClientClient>(value.clone()).is_err(),
            "fixture must reproduce the failure before the backfill runs"
        );
        backfill_clerk_client_json(&mut value);
        serde_json::from_value::<ClientClient>(value).expect(
            "a signed-in user with no attempt in flight is the COMMON case, not an edge \
             one - this is the payload that broke session persistence in production",
        );
    }

    /// The replacement must spell out all four keys: they are `Option` but
    /// carry `deserialize_with = "Option::deserialize"`, so an empty object
    /// fails with `missing field`. This is the trap that makes `json!({})`
    /// look correct.
    #[test]
    fn sign_up_verifications_replacement_is_not_an_empty_object() {
        serde_json::from_value::<clerk_fapi_rs::models::ClientSignUpVerifications>(
            empty_sign_up_verifications(),
        )
        .expect("the replacement must deserialize into ClientSignUpVerifications");
        assert!(
            serde_json::from_value::<clerk_fapi_rs::models::ClientSignUpVerifications>(json!({}))
                .is_err(),
            "if an empty object now works, empty_sign_up_verifications can be simplified"
        );
    }

    /// A `verifications` object guest-js DID populate must survive untouched -
    /// the rewrite is for null/absent only.
    #[test]
    fn backfill_never_overwrites_a_populated_sign_up_verifications() {
        let mut value = json!({
            "object": "sign_up",
            "verifications": { "email_address": { "sentinel": true } },
        });
        backfill_clerk_client_json(&mut value);
        assert_eq!(
            value["verifications"]["email_address"]["sentinel"],
            json!(true)
        );
    }

    #[test]
    fn backfill_never_overwrites_a_non_null_sign_in_or_sign_up_status() {
        let mut value = json!({
            "object": "client",
            "sign_in": { "object": "sign_in", "status": "needs_first_factor" },
            "sign_up": { "object": "sign_up", "status": "missing_requirements" },
        });
        backfill_clerk_client_json(&mut value);
        assert_eq!(value["sign_in"]["status"], json!("needs_first_factor"));
        assert_eq!(value["sign_up"]["status"], json!("missing_requirements"));
    }

    /// Every object type whose `status` clerk-fapi-rs types as a bare enum
    /// must have a fallback, and no other type may acquire one by accident -
    /// an over-broad rule would invent a `status` on objects that have none.
    #[test]
    fn only_the_bare_enum_status_types_get_a_fallback() {
        assert_eq!(null_status_fallback("session"), Some("active"));
        assert_eq!(null_status_fallback("sign_in_attempt"), Some("abandoned"));
        assert_eq!(null_status_fallback("sign_up_attempt"), Some("abandoned"));
        for other in ["client", "user", "email_address", "token", "organization"] {
            assert_eq!(null_status_fallback(other), None, "{other}");
        }
        // The fallback is keyed on the CANONICAL type, so the raw guest-js
        // spellings must not match - they are rewritten first.
        assert_eq!(null_status_fallback("sign_in"), None);
        assert_eq!(null_status_fallback("sign_up"), None);
    }
}

#[cfg(test)]
mod backfill_gap_tests {
    use super::*;

    /// The rename `has_password` -> `password_enabled` is documented in
    /// `defaults_for`, but the default there is a flat `false`. A sign-up that
    /// DID set a password says so, and defaulting past it writes the opposite
    /// of the truth into the cached session.
    #[test]
    fn a_real_has_password_is_carried_over_not_defaulted_to_false() {
        let mut v = json!({ "object": "sign_up_attempt", "has_password": true });
        backfill_clerk_client_json(&mut v);
        assert_eq!(v["password_enabled"], json!(true));
    }

    #[test]
    fn has_password_false_still_reads_false() {
        let mut v = json!({ "object": "sign_up_attempt", "has_password": false });
        backfill_clerk_client_json(&mut v);
        assert_eq!(v["password_enabled"], json!(false));
    }

    /// Neither key present: the existing `false` default still applies.
    #[test]
    fn absent_on_both_sides_keeps_the_false_default() {
        let mut v = json!({ "object": "sign_up_attempt" });
        backfill_clerk_client_json(&mut v);
        assert_eq!(v["password_enabled"], json!(false));
    }

    /// A present-but-non-BOOLEAN `password_enabled` is unusable: the field is a
    /// bare `bool` upstream, so it fails deserialization exactly like a missing
    /// key - and `defaults_for`'s `or_insert` cannot rescue it, because the key
    /// exists. It must be normalized, not left alone.
    #[test]
    fn a_non_boolean_password_enabled_is_normalized() {
        for bad in [Value::Null, json!("yes"), json!(1)] {
            let mut v = json!({
                "object": "sign_up_attempt",
                "password_enabled": bad,
                "has_password": true,
            });
            backfill_clerk_client_json(&mut v);
            assert_eq!(v["password_enabled"], json!(true), "from {v:?}");
        }
    }

    /// ...and with no usable `has_password` either, it falls back to `false`
    /// rather than staying a value that cannot deserialize.
    #[test]
    fn a_non_boolean_password_enabled_falls_back_to_false() {
        let mut v = json!({ "object": "sign_up_attempt", "password_enabled": Value::Null });
        backfill_clerk_client_json(&mut v);
        assert_eq!(v["password_enabled"], json!(false));
    }

    /// An explicit `password_enabled` is authoritative and must not be
    /// overwritten by `has_password`.
    #[test]
    fn an_explicit_password_enabled_wins() {
        let mut v = json!({
            "object": "sign_up_attempt",
            "password_enabled": true,
            "has_password": false,
        });
        backfill_clerk_client_json(&mut v);
        assert_eq!(v["password_enabled"], json!(true));
    }

    /// The gap this closes: only a NON-object `verifications` used to be
    /// replaced, so a partial object kept its missing keys - and every field
    /// carries `deserialize_with = "Option::deserialize"`, which needs the key
    /// PRESENT even for a null. A partial object fails exactly like `{}` does.
    #[test]
    fn a_partial_verifications_object_is_completed_not_left_short() {
        let mut v = json!({
            "object": "sign_up_attempt",
            "verifications": { "email_address": { "status": "verified" } },
        });
        backfill_clerk_client_json(&mut v);

        let ver = v["verifications"].as_object().unwrap();
        for key in [
            "email_address",
            "phone_number",
            "web3_wallet",
            "external_account",
        ] {
            assert!(ver.contains_key(key), "missing {key}: {ver:?}");
        }
        // The populated entry survives untouched.
        assert_eq!(ver["email_address"]["status"], json!("verified"));
        assert_eq!(ver["phone_number"], Value::Null);
    }

    /// The pre-existing cases must keep working: null and absent both become
    /// the full all-null skeleton.
    #[test]
    fn a_null_or_absent_verifications_still_becomes_the_full_skeleton() {
        for mut v in [
            json!({ "object": "sign_up_attempt", "verifications": Value::Null }),
            json!({ "object": "sign_up_attempt" }),
        ] {
            backfill_clerk_client_json(&mut v);
            let ver = v["verifications"].as_object().unwrap();
            assert_eq!(ver.len(), 4, "{ver:?}");
        }
    }
}
