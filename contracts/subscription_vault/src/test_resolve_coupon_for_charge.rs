//! Adversarial coverage for `coupon::resolve_coupon_for_charge`.
//!
//! `resolve_coupon_for_charge` (`contracts/subscription_vault/src/coupon.rs:253`)
//! is the first step of the charge-time coupon path and the only place the
//! subscription → coupon link is turned back into a `Coupon`:
//!
//! ```text
//! pub fn resolve_coupon_for_charge(env: &Env, subscription_id: u32) -> Option<Coupon> {
//!     let code: Symbol = env
//!         .storage()
//!         .persistent()
//!         .get(&DataKey::SubCoupon(subscription_id))?;
//!     read_coupon(env, &code)
//! }
//! ```
//!
//! It is a two-hop **persistent-storage** lookup — `SubCoupon(id) -> Symbol`
//! then `Coupon(code) -> Coupon` — with no `Result`, no caller and no clock.
//! Its whole observable contract is "what came back", and its only production
//! consumer (`coupon::apply_discount_at_charge`, called from
//! `charge_core::charge_one` at `charge_core.rs:348`) turns *every* `Some` into
//! real money movement. So the properties pinned here are the ones a careless
//! one-line change could silently move money with.
//!
//! * **Two hops, both required** — a binding whose target record is missing is a
//!   *dangling* link and must read as `None` (charge at gross) rather than abort
//!   the charge or invent a default coupon. A `Coupon` record with no binding is
//!   likewise not a discount.
//! * **The record is returned as stored** — `resolve` returns whatever is under
//!   `Coupon(code)` verbatim. It does not re-key it, does not rewrite
//!   `coupon.code` to match the binding and does not cross-check the two. A
//!   record stored under code `A` whose `code` field says `B` resolves to a
//!   coupon that identifies itself as `B`, and that `B` then flows into the
//!   `discount_applied` event.
//! * **Snapshot, not a live view and not a handle** — each call re-reads both
//!   slots (no caching, so a `revoke_coupon` is visible on the very next read),
//!   and the returned `Coupon` is an owned copy: mutating it cannot write back.
//! * **No filtering** — revoked, expired, limit-exhausted and token-mismatched
//!   coupons all still resolve as `Some`, with their flags intact. That gating
//!   belongs to `validate_coupon_for_charge`, and it is the *only* thing standing
//!   between a revoked coupon and a discount. A "helpful" filter added here would
//!   quietly change billing.
//! * **Clock-independent** — identical result at timestamp `0` and at
//!   `u64::MAX`, across the `expires_at` boundary: `resolve` never reads
//!   `env.ledger()`.
//! * **Unscoped by subscription** — the key is a bare `u32`. A binding for an id
//!   with no `DataKey::Sub` record resolves normally: `resolve` consults no
//!   subscription, no subscriber and no merchant.
//! * **Purity** — no writes, no event, no authorization consumed and (unlike
//!   every writer in the module) **no TTL extension**: reading a coupon does not
//!   keep it alive.
//! * **Tier confinement** — only persistent storage is consulted, so instance and
//!   temporary entries under the identical keys are invisible.
//! * **Type safety** — a value of the wrong type under either key aborts with
//!   `ConversionError` rather than being coerced.
//!
//! ## Authorization
//!
//! The function takes no `Address`, so the ABI cannot distinguish an admin from a
//! stranger and there is nothing to reject. The property that actually holds is
//! pinned instead: *no authorization is demanded and none is recorded, for any
//! caller* — proved against `env.mock_auths(&[])` (strict mode: any
//! `require_auth` would abort) plus `env.auths()`.
//!
//! No public contract is changed here: no entrypoint is added, removed or
//! re-signed, and `resolve_coupon_for_charge` keeps its signature.

#![cfg(test)]

use crate::coupon::{
    apply_discount_at_charge, compute_discount, get_coupon, increment_redemptions,
    resolve_coupon_for_charge,
};
use crate::test_utils::fixtures;
use crate::types::{
    Coupon, DataKey, DiscountAppliedEvent, Error, SUB_TTL_EXTEND_TO, SUB_TTL_THRESHOLD,
};
use crate::{SubscriptionStatus, SubscriptionVault, SubscriptionVaultClient};
use soroban_sdk::{
    testutils::{
        storage::Persistent as _, Address as _, Events as _, Ledger as _, MockAuth, MockAuthInvoke,
    },
    token, Address, BytesN, Env, FromVal, IntoVal, String as SorobanString, Symbol, Vec,
};

const T0: u64 = 1_000_000;
const AMOUNT: i128 = 10_000_000;
const INTERVAL: u64 = 30 * 24 * 60 * 60;
const GRACE_PERIOD: u64 = 7 * 24 * 60 * 60;

/// The persistent-storage slot holding the subscription → code link. Restated
/// here so a rename of the variant surfaces as a test failure instead of as a
/// silently orphaned binding.
fn binding_key(id: u32) -> DataKey {
    DataKey::SubCoupon(id)
}

/// The persistent-storage slot holding the coupon record.
fn record_key(code: &Symbol) -> DataKey {
    DataKey::Coupon(code.clone())
}

// ── Fixtures ──────────────────────────────────────────────────────────────────

/// A deployed vault with a real Stellar Asset Contract, so the charge path can
/// actually move funds.
struct Fx {
    env: Env,
    contract_id: Address,
    client: SubscriptionVaultClient<'static>,
    admin: Address,
    token: Address,
}

impl Fx {
    fn token_client(&self) -> token::StellarAssetClient<'static> {
        token::StellarAssetClient::new(&self.env, &self.token)
    }

    /// A coupon owned by this fixture's admin: 10 % + 50 fixed units.
    fn coupon(&self, code: &Symbol) -> Coupon {
        Coupon {
            code: code.clone(),
            merchant: self.admin.clone(),
            token: self.token.clone(),
            percent_off_bps: 1_000,
            fixed_off: 50,
            max_redemptions: 10,
            expires_at: 0,
            revoked: false,
        }
    }

    /// Plant a binding **and** its record with raw storage writes, bypassing both
    /// `apply_coupon` and `write_coupon` — so neither slot's TTL is extended.
    fn plant_binding(&self, id: u32, code: &Symbol) -> Coupon {
        let coupon = self.coupon(code);
        self.env.as_contract(&self.contract_id, || {
            self.env
                .storage()
                .persistent()
                .set(&record_key(code), &coupon);
            self.env.storage().persistent().set(&binding_key(id), code);
        });
        coupon
    }

    /// Point `id` at `code` without providing a record: the dangling-link state.
    fn plant_dangling(&self, id: u32, code: &Symbol) {
        self.env.as_contract(&self.contract_id, || {
            self.env.storage().persistent().set(&binding_key(id), code)
        });
    }
}

fn setup() -> Fx {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();

    client.init(&token, &6, &admin, &1_000_000i128, &GRACE_PERIOD);
    Fx {
        env,
        contract_id,
        client,
        admin,
        token,
    }
}

/// The function under test. Persistent storage is only reachable from inside the
/// contract's own context, so every call is wrapped in `env.as_contract`.
fn resolve(env: &Env, contract_id: &Address, id: u32) -> Option<Coupon> {
    env.as_contract(contract_id, || resolve_coupon_for_charge(env, id))
}

/// The same read made through the only non-test consumer.
fn charge_path(
    env: &Env,
    contract_id: &Address,
    id: u32,
    now: u64,
    token: &Address,
    gross: i128,
) -> (i128, i128) {
    env.as_contract(contract_id, || {
        apply_discount_at_charge(env, id, now, token, gross)
    })
}

/// The `discount_applied` events published so far — the charge path's only
/// externally-visible proof that a discount was actually taken.
fn discount_events(env: &Env) -> std::vec::Vec<DiscountAppliedEvent> {
    let topic = Symbol::new(env, "discount_applied");
    let mut out = std::vec::Vec::new();
    for (_, topics, data) in env.events().all().into_iter() {
        if topics
            .get(0)
            .map(|t| Symbol::from_val(env, &t) == topic)
            .unwrap_or(false)
        {
            out.push(DiscountAppliedEvent::from_val(env, &data));
        }
    }
    out
}

// ── Happy path: both hops present ─────────────────────────────────────────────

#[test]
fn resolves_the_bound_coupon_for_a_bound_subscription() {
    let fx = setup();
    let code = Symbol::new(&fx.env, "BOUND");
    let planted = fx.plant_binding(1, &code);

    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 1),
        Some(planted.clone()),
        "a binding plus a matching record must resolve to the stored coupon"
    );
    // ...and the second hop really is the record, not a reconstruction from the
    // binding: the public getter sees the same value.
    assert_eq!(
        fx.env
            .as_contract(&fx.contract_id, || get_coupon(&fx.env, code.clone())),
        Some(planted)
    );
}

#[test]
fn every_field_of_the_record_round_trips_unmodified() {
    // No field is defaulted, clamped or dropped on the way out. In particular
    // `percent_off_bps > 10_000`, a negative `fixed_off`, `expires_at = u64::MAX`
    // and `max_redemptions = u32::MAX` must all survive verbatim: `resolve`
    // reports what is stored and leaves the judgement to `compute_discount` and
    // `validate_coupon_for_charge`.
    let fx = setup();
    let code = Symbol::new(&fx.env, "EXTREME");
    let mut planted = fx.coupon(&code);
    planted.percent_off_bps = u32::MAX;
    planted.fixed_off = i128::MIN;
    planted.max_redemptions = u32::MAX;
    planted.expires_at = u64::MAX;

    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &planted);
        fx.env.storage().persistent().set(&binding_key(9), &code);
    });

    let resolved = resolve(&fx.env, &fx.contract_id, 9).expect("must resolve");
    assert_eq!(resolved, planted);
    assert_eq!(resolved.percent_off_bps, u32::MAX);
    assert_eq!(resolved.fixed_off, i128::MIN);
    assert_eq!(resolved.max_redemptions, u32::MAX);
    assert_eq!(resolved.expires_at, u64::MAX);
}

#[test]
fn the_code_field_is_returned_as_stored_not_rewritten_to_the_binding() {
    // The two hops are never cross-checked. A record stored under code `A` whose
    // `code` field says `B` resolves to a coupon that identifies itself as `B`,
    // while `get_coupon(B)` still reports "no such coupon". This is exactly the
    // state a hand-edited or migrated storage layout produces, and the charge
    // path propagates it verbatim into the `discount_applied` event
    // (`coupon_code: coupon.code`) — see
    // `an_aliased_record_reports_its_own_code_in_the_discount_event`.
    let fx = setup();
    let stored_under = Symbol::new(&fx.env, "STOREDKEY");
    let says = Symbol::new(&fx.env, "SAYSOTHER");

    let mut record = fx.coupon(&stored_under);
    record.code = says.clone();
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&stored_under), &record);
        fx.env
            .storage()
            .persistent()
            .set(&binding_key(3), &stored_under);
    });

    let resolved = resolve(&fx.env, &fx.contract_id, 3).expect("must resolve");
    assert_eq!(resolved.code, says, "the stored `code` field wins");
    assert_eq!(
        fx.env
            .as_contract(&fx.contract_id, || get_coupon(&fx.env, says.clone())),
        None,
        "nothing is keyed by the reported code"
    );
}

// ── Dangling links and absence ────────────────────────────────────────────────

#[test]
fn returns_none_when_the_subscription_has_no_binding() {
    let fx = setup();
    let code = Symbol::new(&fx.env, "ORPHAN");
    fx.plant_binding(1, &code);

    for id in [0u32, 2, 4_242, u32::MAX - 1, u32::MAX] {
        assert_eq!(
            resolve(&fx.env, &fx.contract_id, id),
            None,
            "id {id} carries no binding"
        );
    }
}

#[test]
fn a_binding_whose_record_is_missing_reads_as_none() {
    // The important asymmetric case: hop one succeeds, hop two misses. The result
    // is `None` — indistinguishable from "no coupon at all" — so the charge path
    // falls through to `(gross, 0)` and billing continues. It must not abort the
    // charge and it must not surface a half-resolved binding.
    let fx = setup();
    let missing = Symbol::new(&fx.env, "GONE");
    fx.plant_dangling(5, &missing);

    assert_eq!(resolve(&fx.env, &fx.contract_id, 5), None);
    assert!(
        !fx.env.as_contract(&fx.contract_id, || fx
            .env
            .storage()
            .persistent()
            .has(&record_key(&missing))),
        "precondition: the record really is absent"
    );
}

#[test]
fn a_coupon_record_on_its_own_is_not_a_discount() {
    // The mirror image: hop two exists, hop one does not. `Coupon` records are
    // created by `create_coupon` and stay resident forever; none of them is
    // active until a subscription binds it.
    let fx = setup();
    for i in 0..5u32 {
        let code = Symbol::new(&fx.env, &format!("REC{i}"));
        let record = fx.coupon(&code);
        fx.env.as_contract(&fx.contract_id, || {
            fx.env
                .storage()
                .persistent()
                .set(&record_key(&code), &record)
        });
    }

    for id in [0u32, 1, 2, 3, 4, 5, u32::MAX] {
        assert_eq!(
            resolve(&fx.env, &fx.contract_id, id),
            None,
            "id {id} must not pick up an unbound record"
        );
    }
}

#[test]
fn removing_either_hop_takes_effect_on_the_next_read() {
    // Both hops are re-read on every call — there is no cache and no shadow key.
    // Deleting the record alone is enough to make an intact binding resolve to
    // `None`, and restoring the record brings the discount straight back without
    // re-binding.
    let fx = setup();
    let code = Symbol::new(&fx.env, "CACHED");
    let planted = fx.plant_binding(2, &code);
    assert!(resolve(&fx.env, &fx.contract_id, 2).is_some());

    fx.env.as_contract(&fx.contract_id, || {
        fx.env.storage().persistent().remove(&record_key(&code))
    });
    assert_eq!(resolve(&fx.env, &fx.contract_id, 2), None);

    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &planted)
    });
    assert_eq!(resolve(&fx.env, &fx.contract_id, 2), Some(planted));
}

#[test]
fn repeated_resolves_are_stable_and_deterministic() {
    let fx = setup();
    let code = Symbol::new(&fx.env, "STABLE");
    let planted = fx.plant_binding(1, &code);

    let first = resolve(&fx.env, &fx.contract_id, 1);
    for i in 0..10 {
        assert_eq!(
            resolve(&fx.env, &fx.contract_id, 1),
            first.clone(),
            "call #{i} drifted"
        );
    }
    assert_eq!(first, Some(planted));
}

// ── Key-space boundaries ──────────────────────────────────────────────────────

#[test]
fn id_zero_is_an_ordinary_key() {
    // `0` is not handed out by `create_subscription`, but it is a perfectly valid
    // `u32` storage key. Neither the resolver nor its key construction treats it
    // as "unset" — which is exactly what a `subscription_id == 0` guard or an
    // `unwrap_or(0)`-style default would do.
    let fx = setup();
    let code = Symbol::new(&fx.env, "ZEROID");
    let planted = fx.plant_binding(0, &code);

    assert_eq!(resolve(&fx.env, &fx.contract_id, 0), Some(planted));
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 1),
        None,
        "id 0 must not spill into id 1"
    );
}

#[test]
fn u32_max_id_is_an_ordinary_key() {
    // Symmetrically, the top of the `u32` range must not wrap, saturate into a
    // neighbouring slot, or be read as "no subscription".
    let fx = setup();
    let code = Symbol::new(&fx.env, "MAXID");
    let planted = fx.plant_binding(u32::MAX, &code);

    assert_eq!(resolve(&fx.env, &fx.contract_id, u32::MAX), Some(planted));
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, u32::MAX - 1),
        None,
        "u32::MAX must not spill into u32::MAX - 1"
    );
}

#[test]
fn neighbouring_ids_do_not_cross_talk() {
    let fx = setup();
    let a = Symbol::new(&fx.env, "ID_A");
    let b = Symbol::new(&fx.env, "ID_B");
    let planted_a = fx.plant_binding(100, &a);
    let planted_b = fx.plant_binding(102, &b);

    assert_eq!(resolve(&fx.env, &fx.contract_id, 100), Some(planted_a));
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 101),
        None,
        "an unbound id between two bound ones must stay None"
    );
    assert_eq!(resolve(&fx.env, &fx.contract_id, 102), Some(planted_b));
}

#[test]
fn a_code_of_the_maximum_symbol_length_resolves() {
    // `Symbol::new` accepts up to 32 characters, far beyond the 9-character
    // "short symbol" range. Long codes are ordinary codes: the second hop must
    // not truncate, hash or reject them.
    let fx = setup();
    let long_code = Symbol::new(&fx.env, "A_VERY_LONG_COUPON_CODE_XYZ_1234");
    assert_eq!(
        long_code.to_string().len(),
        32,
        "precondition: a maximum-length symbol"
    );
    let planted = fx.plant_binding(1, &long_code);

    assert_eq!(resolve(&fx.env, &fx.contract_id, 1), Some(planted));
}

#[test]
fn resolve_never_consults_the_subscription_record() {
    // `resolve` reads exactly two slots and neither is `DataKey::Sub`. A binding
    // for an id with no subscription still resolves, and a deleted subscription
    // does not invalidate its binding. Pinned because "look the subscription up
    // first" is the natural refactor for a reader who assumes a dangling id is
    // impossible — and it would turn every orphan binding from `None` into a
    // hard error.
    let fx = setup();
    let code = Symbol::new(&fx.env, "NOSUB");
    let planted = fx.plant_binding(4_242, &code);

    assert_eq!(
        fx.client.try_get_subscription(&4_242),
        Err(Ok(Error::NotFound)),
        "precondition: no subscription exists for 4242"
    );
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 4_242),
        Some(planted),
        "resolve must not require a subscription record"
    );

    // A real subscription whose record is then deleted keeps resolving.
    let (real_id, _sub, _merchant) = fixtures::create_subscription_detailed(
        &fx.env,
        &fx.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    let second = Symbol::new(&fx.env, "SECOND");
    let planted_second = fx.plant_binding(real_id, &second);
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, real_id),
        Some(planted_second.clone())
    );

    fx.env.as_contract(&fx.contract_id, || {
        fx.env.storage().persistent().remove(&DataKey::Sub(real_id))
    });
    assert_eq!(
        fx.client.try_get_subscription(&real_id),
        Err(Ok(Error::NotFound)),
        "precondition: the subscription is gone"
    );
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, real_id),
        Some(planted_second),
        "the binding outlives the subscription record"
    );
}

// ── No filtering: the gating belongs to validate_coupon_for_charge ────────────

#[test]
fn a_revoked_coupon_still_resolves_with_the_flag_intact() {
    // A revoked coupon *is* resolved; `validate_coupon_for_charge` is what turns
    // `Some(revoked)` into a silent skip. If a filter were ever added here, the
    // distinguishing evidence — `revoked: true` reaching the validator — would
    // vanish and a revoked coupon would look like an absent one.
    let fx = setup();
    let code = Symbol::new(&fx.env, "REVOKED");
    let mut record = fx.coupon(&code);
    record.revoked = true;
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &record);
        fx.env.storage().persistent().set(&binding_key(1), &code);
    });

    assert!(
        resolve(&fx.env, &fx.contract_id, 1)
            .expect("must resolve")
            .revoked,
        "the revoked flag must survive the read"
    );
}

#[test]
fn revoke_coupon_is_visible_to_the_next_resolve() {
    // Driven through the public ABI so this proves the live writer and the
    // resolver agree, not just two raw storage writes agreeing.
    let fx = setup();
    let merchant = fx.admin.clone();
    let code = Symbol::new(&fx.env, "LIVEREVOKE");
    fx.client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &fx.token, &2_000, &0, &0, &0);
    let (id, subscriber, sub_merchant) = fixtures::create_subscription_with_merchant(
        &fx.env,
        &fx.client,
        SubscriptionStatus::Active,
        merchant.clone(),
    );
    assert_eq!(sub_merchant, merchant, "precondition: one merchant");

    fx.client
        .mock_all_auths()
        .apply_coupon(&subscriber, &id, &code);
    assert!(
        !resolve(&fx.env, &fx.contract_id, id)
            .expect("bound")
            .revoked,
        "precondition: the coupon is live"
    );

    fx.client.mock_all_auths().revoke_coupon(&merchant, &code);
    assert!(
        resolve(&fx.env, &fx.contract_id, id)
            .expect("still bound")
            .revoked,
        "revocation must be visible on the very next read"
    );
}

#[test]
fn an_expired_coupon_still_resolves() {
    let fx = setup();
    let code = Symbol::new(&fx.env, "EXPIRED");
    let mut record = fx.coupon(&code);
    record.expires_at = T0 - 1;
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &record);
        fx.env.storage().persistent().set(&binding_key(1), &code);
    });

    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 1)
            .expect("must resolve")
            .expires_at,
        T0 - 1
    );
}

#[test]
fn resolve_is_independent_of_the_ledger_clock() {
    // `resolve` never reads `env.ledger()`. Sweeping the timestamp from 0 to
    // `u64::MAX` — across the `expires_at` boundary — must not change the answer,
    // which pins the expiry gate to the validator instead of leaking it into the
    // lookup.
    let fx = setup();
    let code = Symbol::new(&fx.env, "CLOCKFREE");
    let mut record = fx.coupon(&code);
    record.expires_at = 500;
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &record);
        fx.env.storage().persistent().set(&binding_key(1), &code);
    });

    let baseline = resolve(&fx.env, &fx.contract_id, 1);
    assert!(baseline.is_some(), "precondition: the binding resolves");
    for ts in [0u64, 1, 499, 500, 501, T0, u64::MAX] {
        fx.env.ledger().with_mut(|l| l.timestamp = ts);
        assert_eq!(
            resolve(&fx.env, &fx.contract_id, 1),
            baseline.clone(),
            "resolution changed at timestamp {ts}"
        );
    }
}

#[test]
fn a_coupon_over_its_redemption_limit_still_resolves() {
    // Redemption limits are enforced at bind time only (deliberately — see the
    // comment in `validate_coupon_for_charge`), so an exhausted coupon must keep
    // resolving for the subscribers that bound it while it was still available.
    let fx = setup();
    let code = Symbol::new(&fx.env, "EXHAUSTED");
    let mut record = fx.coupon(&code);
    record.max_redemptions = 1;
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &record);
        fx.env.storage().persistent().set(&binding_key(1), &code);
        fx.env
            .storage()
            .persistent()
            .set(&DataKey::CouponRedemptions(code.clone()), &1u32);
    });

    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 1)
            .expect("must resolve")
            .max_redemptions,
        1
    );
}

#[test]
fn a_coupon_for_another_token_still_resolves() {
    // `apply_coupon` refuses a token mismatch, so this state is only reachable
    // through a storage edit — but it is exactly what a merchant-side migration
    // produces, and it must read as a resolvable coupon whose *token* differs
    // from the subscription's, not as `None`. The token check belongs to
    // `validate_coupon_for_charge`.
    let fx = setup();
    let code = Symbol::new(&fx.env, "WRONGTOKEN");
    let mut record = fx.coupon(&code);
    record.token = Address::generate(&fx.env);
    let foreign = record.token.clone();
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &record);
        fx.env.storage().persistent().set(&binding_key(1), &code);
    });

    let resolved = resolve(&fx.env, &fx.contract_id, 1).expect("must resolve");
    assert_eq!(resolved.token, foreign);
    assert_ne!(resolved.token, fx.token, "precondition: tokens differ");
}

// ── Snapshot semantics ────────────────────────────────────────────────────────

#[test]
fn mutating_the_returned_coupon_cannot_write_back_to_storage() {
    // The return value is an owned `Coupon`, not a handle onto the stored record.
    // If it aliased, a caller normalising the resolved coupon would silently
    // rewrite contract state without `write_coupon`, without a TTL extension and
    // without an event.
    let fx = setup();
    let code = Symbol::new(&fx.env, "NOSHARE");
    let planted = fx.plant_binding(1, &code);

    let mut resolved = resolve(&fx.env, &fx.contract_id, 1).expect("must resolve");
    resolved.revoked = true;
    resolved.percent_off_bps = 10_000;
    resolved.fixed_off = i128::MAX;
    resolved.max_redemptions = 0;
    resolved.expires_at = 1;

    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 1),
        Some(planted),
        "the stored record must be untouched by the caller's mutations"
    );
}

#[test]
fn an_earlier_result_is_not_retroactively_updated_by_a_later_write() {
    // The complement: each call returns a snapshot taken at call time, so
    // rewriting the record afterwards must not change a value already returned.
    let fx = setup();
    let code = Symbol::new(&fx.env, "SNAPSHOT");
    fx.plant_binding(1, &code);
    let first = resolve(&fx.env, &fx.contract_id, 1).expect("must resolve");

    let mut rewritten = fx.coupon(&code);
    rewritten.percent_off_bps = 7_500;
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &rewritten)
    });

    assert_eq!(first.percent_off_bps, 1_000, "the snapshot is frozen");
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 1)
            .expect("still bound")
            .percent_off_bps,
        7_500,
        "and the next call sees the new value"
    );
}

// ── Purity ────────────────────────────────────────────────────────────────────

#[test]
fn resolve_demands_no_authorization_and_records_none() {
    // No caller to reject: the observable contract is "nothing demanded, nothing
    // recorded". `mock_auths(&[])` is strict mode — any `require_auth` would
    // abort rather than silently succeed.
    let env = Env::default();
    let contract_id = env.register(SubscriptionVault, ());
    env.mock_auths(&[]);

    let code = Symbol::new(&env, "NOAUTH");
    let owner = Address::generate(&env);
    let record = Coupon {
        code: code.clone(),
        merchant: owner.clone(),
        token: owner,
        percent_off_bps: 1_000,
        fixed_off: 0,
        max_redemptions: 0,
        expires_at: 0,
        revoked: false,
    };
    env.as_contract(&contract_id, || {
        env.storage().persistent().set(&record_key(&code), &record);
        env.storage().persistent().set(&binding_key(1), &code);
    });
    env.set_auths(&[]);

    assert!(
        resolve(&env, &contract_id, 1).is_some(),
        "the read still succeeds"
    );
    assert!(
        discount_events(&env).is_empty(),
        "resolving a coupon must not emit an event"
    );
    assert!(
        env.auths().is_empty(),
        "resolving a coupon must not consume an authorization entry"
    );
}

#[test]
fn resolving_an_unbound_subscription_writes_nothing_and_emits_nothing() {
    // A `None` read must not materialize the binding slot, materialize a record
    // slot, or publish anything. A lazily-inserted default would turn "no coupon"
    // into a real storage entry on every charge.
    let fx = setup();
    let absent = Symbol::new(&fx.env, "NEVERWRITTEN");

    for _ in 0..8 {
        assert_eq!(resolve(&fx.env, &fx.contract_id, 77), None);
    }

    fx.env.as_contract(&fx.contract_id, || {
        assert!(
            !fx.env.storage().persistent().has(&binding_key(77)),
            "the binding slot must stay absent"
        );
        assert!(
            !fx.env.storage().persistent().has(&record_key(&absent)),
            "the record slot must stay absent"
        );
    });
    assert!(
        discount_events(&fx.env).is_empty(),
        "a None read must be silent"
    );
}

#[test]
fn resolve_leaves_both_slots_byte_identical() {
    let fx = setup();
    let code = Symbol::new(&fx.env, "PURE");
    fx.plant_binding(1, &code);

    let snap = |fx: &Fx| {
        fx.env.as_contract(&fx.contract_id, || {
            (
                fx.env
                    .storage()
                    .persistent()
                    .get::<_, Symbol>(&binding_key(1)),
                fx.env
                    .storage()
                    .persistent()
                    .get::<_, Coupon>(&record_key(&code)),
                fx.env
                    .storage()
                    .persistent()
                    .get::<_, u32>(&DataKey::CouponRedemptions(code.clone())),
            )
        })
    };
    let before = snap(&fx);
    assert_eq!(before.2, None, "precondition: no redemption counter");

    for _ in 0..5 {
        assert!(resolve(&fx.env, &fx.contract_id, 1).is_some());
    }

    assert_eq!(
        snap(&fx),
        before,
        "resolution must not mutate or fabricate anything"
    );
}

#[test]
fn resolve_does_not_extend_the_ttl_of_either_slot() {
    // Every writer in this module calls `maybe_extend_ttl` with
    // `SUB_TTL_THRESHOLD` / `SUB_TTL_EXTEND_TO`; the resolver deliberately does
    // not, because it is a read. That is what keeps a view call free of storage
    // mutation and of the ledger-entry fee an extension can incur. Asserted
    // against the raw TTLs, with a control proving the assertion is not vacuous:
    // the very same `extend_ttl` call *does* move them.
    let fx = setup();
    let code = Symbol::new(&fx.env, "NOTTL");
    fx.plant_binding(1, &code);

    let ttls = |fx: &Fx| {
        fx.env.as_contract(&fx.contract_id, || {
            (
                fx.env.storage().persistent().get_ttl(&binding_key(1)),
                fx.env.storage().persistent().get_ttl(&record_key(&code)),
            )
        })
    };

    let before = ttls(&fx);
    assert!(
        before.0 < SUB_TTL_THRESHOLD,
        "precondition: the planted entries sit below the extension threshold (ttl = {})",
        before.0
    );

    for _ in 0..5 {
        assert!(resolve(&fx.env, &fx.contract_id, 1).is_some());
    }
    assert_eq!(
        ttls(&fx),
        before,
        "reading a coupon must not extend the ttl of the binding or the record"
    );

    // Control: the identical extension the writers perform would have moved
    // these TTLs, so the assertion above is really observing something.
    fx.env.as_contract(&fx.contract_id, || {
        fx.env.storage().persistent().extend_ttl(
            &binding_key(1),
            SUB_TTL_THRESHOLD,
            SUB_TTL_EXTEND_TO,
        );
    });
    assert!(
        ttls(&fx).0 > before.0,
        "control: extend_ttl must move the binding's ttl"
    );
}

// ── Storage-tier confinement ──────────────────────────────────────────────────

#[test]
fn bindings_and_records_in_other_tiers_are_invisible() {
    // Both hops read **persistent** storage. A binding planted in instance
    // storage, or a record planted in temporary storage, under the identical keys
    // must not be picked up — the mistake a tier-fallback reader would make.
    let fx = setup();
    let code = Symbol::new(&fx.env, "TIERS");
    let temporary_record = fx.coupon(&code);

    fx.env.as_contract(&fx.contract_id, || {
        fx.env.storage().instance().set(&binding_key(1), &code);
        fx.env
            .storage()
            .temporary()
            .set(&record_key(&code), &temporary_record);
    });
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 1),
        None,
        "an instance binding must be invisible"
    );

    // Both hops present, but only the binding is in the wrong tier.
    let persistent_record = fx.coupon(&code);
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &persistent_record);
    });
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 1),
        None,
        "an instance binding plus a persistent record is still dangling"
    );

    // ...and the persistent binding now completes the chain.
    fx.env.as_contract(&fx.contract_id, || {
        fx.env.storage().persistent().set(&binding_key(1), &code)
    });
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, 1),
        Some(persistent_record)
    );
}

#[test]
fn the_binding_slot_and_the_record_slot_are_distinct_keys() {
    // `DataKey::SubCoupon(_)` and `DataKey::Coupon(_)` are different variants
    // (discriminants 69 and 57). If they ever shared a shape — or the
    // `Coupon` record moved onto the `SubCoupon` key — hop one would read a
    // `Coupon` as a code and every binding would break. Both slots are populated
    // here and each id still resolves to exactly its own record.
    let fx = setup();
    assert_eq!(binding_key(1).canonical_discriminant(), 69);
    assert_eq!(
        record_key(&Symbol::new(&fx.env, "x")).canonical_discriminant(),
        57
    );

    let one = Symbol::new(&fx.env, "ONE");
    let two = Symbol::new(&fx.env, "TWO");
    let planted_one = fx.plant_binding(1, &one);
    let planted_two = fx.plant_binding(2, &two);

    fx.env.as_contract(&fx.contract_id, || {
        assert!(fx.env.storage().persistent().has(&binding_key(1)));
        assert!(fx.env.storage().persistent().has(&binding_key(2)));
        assert!(fx.env.storage().persistent().has(&record_key(&one)));
        assert!(fx.env.storage().persistent().has(&record_key(&two)));
    });
    assert_eq!(resolve(&fx.env, &fx.contract_id, 1), Some(planted_one));
    assert_eq!(resolve(&fx.env, &fx.contract_id, 2), Some(planted_two));
}

// ── Type safety ───────────────────────────────────────────────────────────────

#[test]
#[should_panic(expected = "ConversionError")]
fn a_coupon_planted_under_the_binding_slot_aborts_the_read() {
    // Hop one asks for a `Symbol`. A `Coupon` left in the binding slot by a
    // mis-migrated layout must abort rather than be coerced — coercing it would
    // synthesize a code nobody bound and silently drop an active discount.
    let fx = setup();
    let code = Symbol::new(&fx.env, "MISPLACED");
    let record = fx.coupon(&code);
    fx.env.as_contract(&fx.contract_id, || {
        fx.env.storage().persistent().set(&binding_key(1), &record)
    });

    let _ = resolve(&fx.env, &fx.contract_id, 1);
}

#[test]
#[should_panic(expected = "ConversionError")]
fn a_string_planted_under_the_binding_slot_aborts_the_read() {
    // `soroban_sdk::String` and `Symbol` are different `Val`s. An older build
    // that keyed the binding by string must not be reinterpreted as a symbol
    // code.
    let fx = setup();
    let as_string = SorobanString::from_str(&fx.env, "STRKEY");
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&binding_key(1), &as_string)
    });

    let _ = resolve(&fx.env, &fx.contract_id, 1);
}

#[test]
#[should_panic(expected = "ConversionError")]
fn a_number_planted_under_the_binding_slot_aborts_the_read() {
    let fx = setup();
    fx.env.as_contract(&fx.contract_id, || {
        fx.env.storage().persistent().set(&binding_key(1), &7u32)
    });

    let _ = resolve(&fx.env, &fx.contract_id, 1);
}

#[test]
#[should_panic(expected = "ConversionError")]
fn a_non_coupon_planted_under_the_record_slot_aborts_the_read() {
    // Hop two asks for a `Coupon`. With the binding intact but the record slot
    // holding a bare number, the second hop must abort rather than reinterpret
    // that number as a discount configuration.
    let fx = setup();
    let code = Symbol::new(&fx.env, "BADREC");
    fx.env.as_contract(&fx.contract_id, || {
        fx.env.storage().persistent().set(&binding_key(1), &code);
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &(1_000u64));
    });

    let _ = resolve(&fx.env, &fx.contract_id, 1);
}

#[test]
// The host rejects the two-entry map before it ever gets to per-field
// conversion, so the observable error is the size mismatch rather than the
// generic `ConversionError` the wrong-type cases above produce.
#[should_panic(expected = "UnexpectedSize")]
fn a_short_struct_planted_under_the_record_slot_aborts_the_read() {
    // The `Coupon` shape is part of the pinned ABI. A record from an older build
    // with fewer fields must abort on decode rather than be padded with defaults
    // that would silently yield a live coupon with no discount.
    let fx = setup();
    let code = Symbol::new(&fx.env, "OLDSHAPE");
    let legacy = LegacyCoupon {
        code: code.clone(),
        merchant: fx.admin.clone(),
    };
    fx.env.as_contract(&fx.contract_id, || {
        fx.env.storage().persistent().set(&binding_key(1), &code);
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&code), &legacy);
    });

    let _ = resolve(&fx.env, &fx.contract_id, 1);
}

/// A `Coupon` shaped like an older, shorter version of the struct: only `code`
/// and `merchant` exist. Decoding it as a modern `Coupon` must fail.
#[derive(Clone, Debug)]
#[soroban_sdk::contracttype]
struct LegacyCoupon {
    code: Symbol,
    merchant: Address,
}

// ── Contract isolation ────────────────────────────────────────────────────────

#[test]
fn bindings_are_scoped_to_one_contract_instance() {
    // Persistent storage is per-contract and `subscription_id` is not globally
    // unique across deployments. If the resolver ever keyed off something that is
    // not tenant-scoped, one deployment could read (or be read by) another's
    // binding for the same numeric id.
    let env = Env::default();
    env.mock_all_auths();
    let a = env.register(SubscriptionVault, ());
    let b = env.register(SubscriptionVault, ());

    let code_a = Symbol::new(&env, "ISO_A");
    let code_b = Symbol::new(&env, "ISO_B");
    let owner = Address::generate(&env);
    let record_a = Coupon {
        code: code_a.clone(),
        merchant: owner.clone(),
        token: owner.clone(),
        percent_off_bps: 1_000,
        fixed_off: 0,
        max_redemptions: 0,
        expires_at: 0,
        revoked: false,
    };
    let mut record_b = record_a.clone();
    record_b.code = code_b.clone();
    record_b.percent_off_bps = 5_000;

    env.as_contract(&a, || {
        env.storage()
            .persistent()
            .set(&record_key(&code_a), &record_a);
        env.storage().persistent().set(&binding_key(1), &code_a);
    });
    env.as_contract(&b, || {
        env.storage()
            .persistent()
            .set(&record_key(&code_b), &record_b);
        env.storage().persistent().set(&binding_key(1), &code_b);
    });

    assert_eq!(resolve(&env, &a, 1), Some(record_a));
    assert_eq!(resolve(&env, &b, 1), Some(record_b.clone()));

    // Clearing one leaves the other intact.
    env.as_contract(&a, || env.storage().persistent().remove(&binding_key(1)));
    assert_eq!(resolve(&env, &a, 1), None);
    assert_eq!(resolve(&env, &b, 1), Some(record_b));
}

#[test]
fn resolve_on_an_uninitialized_vault_is_none_and_not_a_panic() {
    // The resolver has no `NotInitialized` guard — it cannot return a `Result` —
    // so on a vault that was registered but never `init`ed it simply reads the
    // (empty) slot. The result must be the same `None` an initialized vault
    // reports for an unbound id: never a panic, never a phantom coupon.
    let env = Env::default();
    env.mock_all_auths();
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    assert_eq!(
        client.try_get_admin(),
        Err(Ok(Error::NotInitialized)),
        "precondition: the vault is not initialized"
    );
    assert_eq!(resolve(&env, &contract_id, 1), None);
}

// ── The charge path: what a `Some` actually buys ──────────────────────────────

#[test]
fn a_none_resolution_charges_gross_and_emits_no_discount_event() {
    let fx = setup();
    assert_eq!(resolve(&fx.env, &fx.contract_id, 1), None);

    let (payable, discount) = charge_path(&fx.env, &fx.contract_id, 1, T0, &fx.token, 10_000);

    assert_eq!((payable, discount), (10_000, 0));
    assert!(
        discount_events(&fx.env).is_empty(),
        "no coupon must mean no discount event"
    );
}

#[test]
fn a_some_resolution_charges_the_discounted_amount_and_emits_the_event() {
    let fx = setup();
    let code = Symbol::new(&fx.env, "APPLIED");
    let planted = fx.plant_binding(1, &code);

    let gross = 10_000i128;
    let (payable, discount) = charge_path(&fx.env, &fx.contract_id, 1, T0, &fx.token, gross);

    assert_eq!(discount, compute_discount(gross, &planted));
    assert_eq!(payable + discount, gross, "gross = discount + payable");
    assert!(discount > 0, "precondition: this coupon really discounts");

    let events = discount_events(&fx.env);
    assert_eq!(events.len(), 1, "exactly one discount event");
    let e = &events[0];
    assert_eq!(e.subscription_id, 1);
    assert_eq!(e.gross_amount, gross);
    assert_eq!(e.discount_amount, discount);
    assert_eq!(e.discounted_amount, payable);
    assert_eq!(e.coupon_code, code);
    assert_eq!(e.timestamp, T0);
}

#[test]
fn an_aliased_record_reports_its_own_code_in_the_discount_event() {
    // `DiscountAppliedEvent.coupon_code` comes from `coupon.code` — from the
    // record, not from the binding slot. An indexer reconciling events against
    // bindings therefore sees `SAYSOTHER` here while the binding says
    // `STOREDKEY`. Pinned because it is the externally-visible half of the
    // no-cross-check property.
    let fx = setup();
    let stored_under = Symbol::new(&fx.env, "STOREDKEY");
    let says = Symbol::new(&fx.env, "SAYSOTHER");
    let mut record = fx.coupon(&stored_under);
    record.code = says.clone();
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&stored_under), &record);
        fx.env
            .storage()
            .persistent()
            .set(&binding_key(1), &stored_under);
    });

    let (payable, discount) = charge_path(&fx.env, &fx.contract_id, 1, T0, &fx.token, 10_000);
    assert!(discount > 0, "the discount applies regardless of the alias");
    assert_eq!(payable + discount, 10_000);

    let events = discount_events(&fx.env);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].coupon_code, says);
}

#[test]
fn a_dangling_binding_charges_gross_and_emits_no_discount_event() {
    // Hop two misses, so the charge path never reaches the validator. The
    // subscriber is billed in full and nothing is published: a broken link must
    // never block billing.
    let fx = setup();
    let missing = Symbol::new(&fx.env, "DANGLING");
    fx.plant_dangling(1, &missing);

    let (payable, discount) = charge_path(&fx.env, &fx.contract_id, 1, T0, &fx.token, 10_000);

    assert_eq!((payable, discount), (10_000, 0));
    assert!(
        discount_events(&fx.env).is_empty(),
        "a broken link must not publish anything"
    );
    assert_eq!(
        fx.env.as_contract(&fx.contract_id, || fx
            .env
            .storage()
            .persistent()
            .get::<_, Symbol>(&binding_key(1))),
        Some(missing),
        "the binding is left as found, so repairing the record is enough"
    );
}

#[test]
fn every_gated_outcome_bills_gross_and_leaves_both_slots_unchanged() {
    // `resolve` reports `Some` for all of these and `validate_coupon_for_charge`
    // is what rejects them, so the charge path silently skips the discount.
    // Pinned as a matrix because "silently" is the requirement: billing must not
    // be blocked, and the binding must survive so a later fix is picked up.
    let cases: [(&str, fn(&Fx, &mut Coupon), bool); 4] = [
        ("revoked", |_, c| c.revoked = true, true),
        ("expired", |_, c| c.expires_at = T0 - 1, true),
        (
            "token_mismatch",
            |fx, c| c.token = Address::generate(&fx.env),
            true,
        ),
        // Redemption limits are deliberately *not* re-checked at charge time, so
        // this one keeps discounting for its existing subscribers.
        ("limit_not_rechecked", |_, c| c.max_redemptions = 1, false),
    ];

    for (label, mutate, gated) in cases {
        let fx = setup();
        let code = Symbol::new(&fx.env, "GATED");
        let mut record = fx.coupon(&code);
        mutate(&fx, &mut record);
        fx.env.as_contract(&fx.contract_id, || {
            fx.env
                .storage()
                .persistent()
                .set(&record_key(&code), &record);
            fx.env.storage().persistent().set(&binding_key(1), &code);
        });

        let snap = |fx: &Fx| {
            fx.env.as_contract(&fx.contract_id, || {
                (
                    fx.env
                        .storage()
                        .persistent()
                        .get::<_, Symbol>(&binding_key(1)),
                    fx.env
                        .storage()
                        .persistent()
                        .get::<_, Coupon>(&record_key(&code)),
                )
            })
        };
        let before = snap(&fx);

        let (payable, discount) = charge_path(&fx.env, &fx.contract_id, 1, T0, &fx.token, 10_000);

        if gated {
            assert_eq!(
                (payable, discount),
                (10_000, 0),
                "{label}: must be billed in full and silently skipped"
            );
            assert!(
                discount_events(&fx.env).is_empty(),
                "{label}: a skipped discount must not be announced"
            );
        } else {
            assert!(
                discount > 0,
                "{label}: the limit must not gate an existing binding"
            );
        }
        assert_eq!(snap(&fx), before, "{label}: slots must be untouched");
    }
}

// ── Rejected operations leave the binding intact ──────────────────────────────

#[test]
fn a_rejected_apply_coupon_leaves_the_existing_binding_resolvable() {
    // A stranger is refused before any write, so the original binding must still
    // resolve to exactly the original coupon — a clobbered binding would silently
    // remove an active discount.
    let fx = setup();
    let original = Symbol::new(&fx.env, "KEEPME");
    let (id, _subscriber, _merchant) = fixtures::create_subscription_detailed(
        &fx.env,
        &fx.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    let planted = fx.plant_binding(id, &original);
    let intruder = Symbol::new(&fx.env, "INTRUDER");
    let mut intruder_record = fx.coupon(&intruder);
    intruder_record.merchant = Address::generate(&fx.env);
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&intruder), &intruder_record)
    });

    let stranger = Address::generate(&fx.env);
    assert_eq!(
        fx.client.try_apply_coupon(&stranger, &id, &intruder),
        Err(Ok(Error::Unauthorized))
    );
    assert_eq!(
        resolve(&fx.env, &fx.contract_id, id),
        Some(planted),
        "the rejected bind must not touch the live binding"
    );
}

#[test]
fn a_rejected_second_binding_attempt_leaves_the_original_resolvable() {
    // `CouponAlreadyApplied` is checked before the new code is even read, so the
    // first code stays bound and resolvable.
    let fx = setup();
    let first = Symbol::new(&fx.env, "FIRST");
    let planted_first = fx.plant_binding(1, &first);
    let second = Symbol::new(&fx.env, "SECOND");
    let second_record = fx.coupon(&second);
    fx.env.as_contract(&fx.contract_id, || {
        fx.env
            .storage()
            .persistent()
            .set(&record_key(&second), &second_record)
    });
    let (id, subscriber, _merchant) = fixtures::create_subscription_detailed(
        &fx.env,
        &fx.client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    // Re-point the planted binding at this real subscription.
    fx.env.as_contract(&fx.contract_id, || {
        fx.env.storage().persistent().set(&binding_key(id), &first)
    });

    assert_eq!(
        fx.client.try_apply_coupon(&subscriber, &id, &second),
        Err(Ok(Error::CouponAlreadyApplied))
    );
    assert_eq!(resolve(&fx.env, &fx.contract_id, id), Some(planted_first));
}

#[test]
fn an_unsigned_charge_leaves_the_binding_resolvable() {
    // The resolver runs *inside* the charge, after `require_stored_admin_auth`. An
    // unsigned charge never reaches it and must not disturb the binding.
    let fx = setup();
    let code = Symbol::new(&fx.env, "UNSIGNED");
    let planted = fx.plant_binding(1, &code);

    fx.env
        .ledger()
        .with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    fx.env.mock_auths(&[]);

    assert!(
        fx.client
            .try_charge_subscription(&1, &None::<BytesN<32>>)
            .is_err(),
        "an unsigned charge must be rejected"
    );
    assert_eq!(resolve(&fx.env, &fx.contract_id, 1), Some(planted));
}

#[test]
fn a_charge_signed_by_a_non_admin_leaves_the_binding_resolvable() {
    // A well-formed signature from the wrong account is not enough: the stored
    // admin must sign.
    let fx = setup();
    let code = Symbol::new(&fx.env, "WRONGSIGNER");
    let planted = fx.plant_binding(1, &code);

    fx.env
        .ledger()
        .with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    let mut args = Vec::new(&fx.env);
    args.push_back(1u32.into_val(&fx.env));
    args.push_back(None::<BytesN<32>>.into_val(&fx.env));
    let stranger = Address::generate(&fx.env);
    let invoke = MockAuthInvoke {
        contract: &fx.contract_id,
        fn_name: "charge_subscription",
        args,
        sub_invokes: &[],
    };
    fx.env.mock_auths(&[MockAuth {
        address: &stranger,
        invoke: &invoke,
    }]);

    assert!(
        fx.client
            .try_charge_subscription(&1, &None::<BytesN<32>>)
            .is_err(),
        "a charge signed by a non-admin must be rejected"
    );
    assert_eq!(resolve(&fx.env, &fx.contract_id, 1), Some(planted));
}

// ── End to end through the real ABI ───────────────────────────────────────────

#[test]
fn a_bound_coupon_reduces_the_charged_amount_end_to_end() {
    // The whole point of the resolver, exercised through the real charge path: a
    // 50 % coupon must leave one and a half periods of runway where an unbound
    // subscription of the same shape leaves one.
    let (left_bound, billed_bound) = run_charge(Some(5_000));
    let (left_unbound, billed_unbound) = run_charge(None);
    assert_eq!(
        billed_bound,
        AMOUNT / 2,
        "a 50 % coupon bills half a period"
    );
    assert_eq!(
        billed_unbound, AMOUNT,
        "without a binding the full period is billed"
    );
    assert_eq!(left_bound, AMOUNT * 2 - AMOUNT / 2);
    assert_eq!(left_unbound, AMOUNT);
    assert!(
        left_bound > left_unbound,
        "the discount must leave more runway"
    );
}

#[test]
fn revoking_after_binding_stops_the_discount_end_to_end() {
    // The documented "already-bound coupons are skipped silently at charge time"
    // behaviour. The binding is *not* cleared: `resolve` keeps returning
    // `Some(revoked)` and `validate_coupon_for_charge` is what drops the discount.
    // So the subscription keeps a coupon slot forever, and un-revoking is the only
    // way to restore the discount.
    let fx = setup();
    let merchant = fx.admin.clone();
    let code = Symbol::new(&fx.env, "E2E_REVOKE");
    fx.client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &fx.token, &5_000, &0, &0, &0);
    let (id, subscriber, sub_merchant) = fixtures::create_subscription_with_merchant(
        &fx.env,
        &fx.client,
        SubscriptionStatus::Active,
        merchant.clone(),
    );
    fx.client
        .mock_all_auths()
        .apply_coupon(&subscriber, &id, &code);
    fx.token_client().mint(&subscriber, &(AMOUNT * 2));
    fx.client
        .mock_all_auths()
        .deposit_funds(&id, &subscriber, &(AMOUNT * 2), &None::<BytesN<32>>);

    fx.env
        .ledger()
        .with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    fx.client
        .mock_all_auths()
        .charge_subscription(&id, &None::<BytesN<32>>);
    // Read the events straight after the charge: the host only surfaces the
    // most recent frame, and any further client call would replace them.
    let first_charge = discount_events(&fx.env);
    assert_eq!(
        first_charge.len(),
        1,
        "precondition: the discounted charge announced itself"
    );
    assert_eq!(first_charge[0].subscription_id, id);
    assert_eq!(first_charge[0].discount_amount, AMOUNT / 2);
    assert_eq!(
        fx.client.get_subscription(&id).prepaid_balance,
        AMOUNT * 2 - AMOUNT / 2,
        "precondition: the first charge was discounted"
    );

    fx.client
        .mock_all_auths()
        .revoke_coupon(&sub_merchant, &code);
    assert!(
        resolve(&fx.env, &fx.contract_id, id)
            .expect("still bound")
            .revoked,
        "the binding survives revocation"
    );

    fx.env
        .ledger()
        .with_mut(|l| l.timestamp = T0 + 2 * INTERVAL + 1);
    fx.client
        .mock_all_auths()
        .charge_subscription(&id, &None::<BytesN<32>>);
    assert!(
        discount_events(&fx.env).is_empty(),
        "the charge after revocation announced no discount"
    );
    assert_eq!(
        fx.client.get_subscription(&id).prepaid_balance,
        AMOUNT * 2 - AMOUNT / 2 - AMOUNT,
        "a revoked coupon must bill the full period"
    );
}

#[test]
fn the_binding_written_by_apply_coupon_resolves_field_for_field() {
    // Round-trips the live writer: what `apply_coupon` persists under
    // `SubCoupon(id)` is exactly what the resolver returns, with nothing dropped
    // or defaulted on the way out.
    let fx = setup();
    let merchant = fx.admin.clone();
    let code = Symbol::new(&fx.env, "ROUNDTRIP");
    fx.client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &fx.token, &3_333, &777, &42, &0);
    let (id, subscriber, sub_merchant) = fixtures::create_subscription_with_merchant(
        &fx.env,
        &fx.client,
        SubscriptionStatus::Active,
        merchant.clone(),
    );
    assert_eq!(sub_merchant, merchant, "precondition: one merchant");

    assert_eq!(resolve(&fx.env, &fx.contract_id, id), None);
    fx.client
        .mock_all_auths()
        .apply_coupon(&subscriber, &id, &code);

    let resolved = resolve(&fx.env, &fx.contract_id, id).expect("must resolve");
    assert_eq!(resolved.code, code);
    assert_eq!(resolved.merchant, merchant);
    assert_eq!(resolved.token, fx.token);
    assert_eq!(resolved.percent_off_bps, 3_333);
    assert_eq!(resolved.fixed_off, 777);
    assert_eq!(resolved.max_redemptions, 42);
    assert_eq!(resolved.expires_at, 0);
    assert!(!resolved.revoked);
    assert_eq!(
        Some(resolved),
        fx.client.mock_all_auths().get_coupon(&code),
        "the resolver and the public getter must agree"
    );
}

#[test]
fn the_resolver_agrees_with_get_coupon_for_every_bound_subscription() {
    // Two independent readers of the same record must never disagree; a
    // divergence would mean the resolver is reading a second, shadow copy.
    let fx = setup();
    let codes: std::vec::Vec<Symbol> = (0..4u32)
        .map(|i| Symbol::new(&fx.env, &format!("RT{i}")))
        .collect();
    for (i, code) in codes.iter().enumerate() {
        let mut record = fx.coupon(code);
        record.percent_off_bps = i as u32 * 1_000;
        fx.env.as_contract(&fx.contract_id, || {
            fx.env
                .storage()
                .persistent()
                .set(&record_key(code), &record);
            fx.env
                .storage()
                .persistent()
                .set(&binding_key(i as u32 + 1), code);
        });
    }

    for (i, code) in codes.iter().enumerate() {
        let id = i as u32 + 1;
        assert_eq!(
            resolve(&fx.env, &fx.contract_id, id),
            fx.env
                .as_contract(&fx.contract_id, || get_coupon(&fx.env, code.clone())),
            "id {id} diverged from get_coupon"
        );
    }
}

#[test]
fn create_coupon_writes_a_record_that_does_not_resolve_until_bound() {
    // `create_coupon` alone must not make any subscription discounted: until
    // `apply_coupon` writes the binding, every id resolves to `None`.
    let fx = setup();
    let merchant = fx.admin.clone();
    let code = Symbol::new(&fx.env, "UNBOUND");
    fx.client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &fx.token, &9_000, &0, &0, &0);

    assert!(
        fx.client.mock_all_auths().get_coupon(&code).is_some(),
        "precondition: the record exists"
    );
    for id in [0u32, 1, 2, 999] {
        assert_eq!(
            resolve(&fx.env, &fx.contract_id, id),
            None,
            "id {id} is not bound"
        );
    }
}

#[test]
fn the_global_redemption_counter_never_influences_resolution() {
    // `CouponRedemptions(code)` is keyed by code and is not one of the two slots
    // `resolve` reads — not even for an exhausted counter.
    let fx = setup();
    let merchant = fx.admin.clone();
    let code = Symbol::new(&fx.env, "COUNTED");
    // A limit of one: binding it spends the single redemption, and nothing else
    // can raise the counter afterwards.
    fx.client
        .mock_all_auths()
        .create_coupon(&merchant, &code, &fx.token, &5_000, &0, &1, &0);

    assert_eq!(resolve(&fx.env, &fx.contract_id, 1), None);
    let (id, subscriber, _) = fixtures::create_subscription_with_merchant(
        &fx.env,
        &fx.client,
        SubscriptionStatus::Active,
        merchant.clone(),
    );
    fx.client
        .mock_all_auths()
        .apply_coupon(&subscriber, &id, &code);

    // ...but once bound, an exhausted counter still does not gate resolution.
    // The counter is shared across every subscription bound to this code, so
    // pushing it past the limit models a coupon that other subscribers burned
    // through. That is the state in which a resolver that consults redemptions
    // would start returning `None`.
    fx.env
        .as_contract(&fx.contract_id, || increment_redemptions(&fx.env, &code));

    let resolved = resolve(&fx.env, &fx.contract_id, id).expect("must resolve");
    assert_eq!(resolved.max_redemptions, 1);
    assert!(compute_discount(10_000, &resolved) > 0);
    assert_eq!(
        fx.env.as_contract(&fx.contract_id, || fx
            .env
            .storage()
            .persistent()
            .get::<_, u32>(&DataKey::CouponRedemptions(code.clone()))),
        Some(2),
        "precondition: the counter really is past the single redemption"
    );
}

/// Deploy a vault with one active subscription, two periods of runway, and
/// optionally a coupon bound to it. Runs exactly one admin charge and returns
/// `(prepaid_balance_after, total_billed)`.
fn run_charge(discount_bps: Option<u32>) -> (i128, i128) {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = T0);

    let admin = Address::generate(&env);
    let contract_id = env.register(SubscriptionVault, ());
    let client = SubscriptionVaultClient::new(&env, &contract_id);
    let token_addr = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let token_client = token::StellarAssetClient::new(&env, &token_addr);
    client.init(&token_addr, &6, &admin, &1_000_000i128, &GRACE_PERIOD);

    let (id, subscriber, merchant) = fixtures::create_subscription_detailed(
        &env,
        &client,
        SubscriptionStatus::Active,
        AMOUNT,
        INTERVAL,
    );
    if let Some(bps) = discount_bps {
        let code = Symbol::new(&env, "E2E");
        client
            .mock_all_auths()
            .create_coupon(&merchant, &code, &token_addr, &bps, &0, &0, &0);
        client
            .mock_all_auths()
            .apply_coupon(&subscriber, &id, &code);
        assert!(
            resolve(&env, &contract_id, id).is_some(),
            "precondition: the coupon is bound"
        );
    }

    token_client.mint(&subscriber, &(AMOUNT * 2));
    client
        .mock_all_auths()
        .deposit_funds(&id, &subscriber, &(AMOUNT * 2), &None::<BytesN<32>>);

    env.ledger().with_mut(|l| l.timestamp = T0 + INTERVAL + 1);
    client
        .mock_all_auths()
        .charge_subscription(&id, &None::<BytesN<32>>);

    let left = client.get_subscription(&id).prepaid_balance;
    (left, (AMOUNT * 2) - left)
}
