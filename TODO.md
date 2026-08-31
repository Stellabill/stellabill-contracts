# TODO

## feat/merchant-kyc-attestation

- [ ] Step 1: Update `contracts/subscription_vault/src/types.rs`
  - Append new `DataKey` variants for `KycRequired` and `MerchantKyc(Address)`.
  - Add `MerchantKyc` struct.
  - Add `MerchantKycAttachedEvent` and `MerchantKycRevokedEvent`.
- [ ] Step 2: Update `contracts/subscription_vault/src/admin.rs`
  - Add admin-only `do_set_kyc_required` and `get_kyc_required` helpers.
- [ ] Step 3: Update `contracts/subscription_vault/src/merchant.rs`
  - Implement `attach_merchant_kyc` + `revoke_merchant_kyc`.
  - Add helper to check active KYC.
  - Gate `withdraw_merchant_funds_for_token` with global `kyc_required`.
- [ ] Step 4: Update `contracts/subscription_vault/src/lib.rs`
  - Add public entrypoints for attaching/revoking KYC and setting global flag.
- [ ] Step 5: Update/add tests in `contracts/subscription_vault/src/test.rs`
  - KYC required off: withdraw unchanged.
  - KYC required on: missing KYC => `Error::Forbidden`.
  - attach -> success
  - revoke -> blocked
  - double-attach rejected
- [ ] Step 6: Run `cargo test --all` and address failures.
- [ ] Step 7: Document in `docs/merchant_config.md` and/or `docs/withdrawals.md`.
- [ ] Step 8: Ensure coverage >= 95% (or add tests until threshold met).
# TODO - feat/merchant-kyc-attestation

- [ ] Create branch `blackboxai/feat/merchant-kyc-attestation`
- [ ] Inspect and update `DataKey` storage layout for merchant KYC + global `kyc_required`
- [ ] Add new types + events + errors (if needed) in `contracts/subscription_vault/src/types.rs`
- [ ] Implement `attach_merchant_kyc` and `revoke_merchant_kyc` in `contracts/subscription_vault/src/merchant.rs`
- [ ] Gate `withdraw_merchant_funds_for_token` when `kyc_required == true`
- [ ] Expose entrypoints in `contracts/subscription_vault/src/lib.rs`
- [ ] Add/extend tests for:
  - [ ] KYC optional (no behavior change)
  - [ ] Missing KYC blocks withdraw with `Error::Forbidden`
  - [ ] Revoke before withdraw blocks
  - [ ] Double-attach rejected
  - [ ] Attach allows withdraw when required
- [ ] Update documentation (`docs/merchant_config.md`)
- [ ] Run `cargo test --all`
- [ ] Ensure coverage >= 95% and commit changes



