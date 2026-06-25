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

