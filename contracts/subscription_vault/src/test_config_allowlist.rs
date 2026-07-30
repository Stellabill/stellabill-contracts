#![cfg(test)]

//! Wave 3 config-key allowlist hardening tests.
//!
//! This file intentionally starts small (partial issue slice) and focuses on
//! security behavior around config mutation entrypoints. Additional assertions
//! for `Error::ConfigKeyNotAllowed` are staged as ignored tests until the
//! enforcement hook is exposed in the runtime write path.

use crate::test_utils::setup::TestEnv;
use crate::types::Error;
use soroban_sdk::testutils::Address as _;
use soroban_sdk::Address;

#[test]
fn config_mutation_path_is_admin_gated() {
    let te = TestEnv::default();
    let non_admin = Address::generate(&te.env);

    let result = te
        .client
        .try_set_min_topup(&non_admin, &2_000_000i128);

    assert_eq!(result, Err(Ok(Error::Forbidden)));
}

#[test]
#[ignore = "Pending wave 3 enforcement: unknown config-key writes should return Error::ConfigKeyNotAllowed"]
fn unlisted_config_key_write_rejected() {
    // TODO(wave3): once config-key allowlist enforcement is wired into the
    // storage write path, add a deterministic write attempt for an unlisted key
    // and assert `Err(Ok(Error::ConfigKeyNotAllowed))`.
}

#[test]
#[ignore = "Pending wave 3 enforcement: add/remove same key lifecycle coverage"]
fn add_then_remove_same_key_lifecycle() {
    // TODO(wave3): exercise allowlist mutation flow with add->remove of the same
    // config key and verify writes pass while listed, then fail after removal.
}

#[test]
#[ignore = "Pending wave 3 enforcement: unicode key label edge-case coverage"]
fn unicode_key_label_edge_case() {
    // TODO(wave3): validate unicode-normalization/byte-sequence behavior for
    // allowlist key labels and ensure matching remains strict and deterministic.
}

#[test]
#[ignore = "Pending wave 3 enforcement: permission escalation attempt coverage"]
fn permission_escalation_attempt_rejected() {
    // TODO(wave3): verify non-admin callers cannot mutate the allowlist and
    // cannot use governance/operator paths to bypass the admin gate.
}
