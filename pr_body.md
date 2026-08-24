## Summary
Post-deployment bug fixes and feature enhancements require a contract upgrade capability. This PR introduces the standard WASM hash rotation mechanism natively supported by the Soroban SDK.

Closes #9

## Implementation Details
The `upgrade` function has been uniformly added across the following core contracts:
- `DatasetRegistry`
- `LicenseRouter`
- `RoyaltySplitter`
- `QualityOracle`
- `DataCommission`

### Security
- The upgrade function fetches the `admin` from instance storage.
- It enforces strict authentication via `admin.require_auth()`.
- Validates the new WASM hash using `env.deployer().update_current_contract_wasm(new_wasm_hash)`.
- Emits an `upgraded` event containing the new WASM hash and ledger sequence to provide transparent auditability on-chain.

## Testing & Verification
All modifications are fully covered by comprehensive unit tests:
- Included a dedicated valid Soroban `dummy_contract` within `test_data` to properly test the WASM rotation without invoking `InvalidInput` or host-side VM parsing panics.
- Included `test_upgrade` to verify that an authorized admin can successfully execute the rotation.
- Included `test_upgrade_unauthorized_not_initialized` to verify that an unauthorized rotation attempt or a call on an uninitialized contract panics as expected.
- All 86 tests passing.

Please let me know if any further tweaks are needed before merging!
