#![no_std]
extern crate alloc;
use alloc::format;
use soroban_sdk::{
    contract, contractclient, contractimpl, contracttype, symbol_short, Address, Env, String,
};

/// Default royalty multiplier (1x, in bps) used when no oracle is
/// configured, or the cross-contract call to it fails for any reason.
const DEFAULT_ROYALTY_MULTIPLIER_BPS: i128 = 10000;

/// How long persistent entries live before they can be archived:
/// 7,776,000 ledgers ≈ 90 days at ~1s/ledger.
const PERSISTENT_TTL: u32 = 7_776_000;

/// Minimal cross-contract interface into QualityOracle — deliberately just
/// the one method this contract needs, rather than depending on the
/// quality-oracle crate directly. Depending on the actual contract crate
/// would pull its own #[contractimpl]-generated WASM exports (initialize,
/// version, ...) into this contract's binary, colliding at link time with
/// license-router's own exports of the same names.
#[contractclient(name = "QualityOracleClient")]
pub trait QualityOracleInterface {
    fn royalty_multiplier_bps(env: Env, dataset_id: String) -> u32;
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LicenseType {
    Research,   // Non-commercial, attribution required
    Commercial, // Full commercial rights
    NonProfit,  // NGO/academic use
    Government, // Government use with audit rights
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LicenseState {
    Active,
    Expired,
    Revoked,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct License {
    pub id: String,
    pub dataset_id: String,
    pub licensee: Address,
    pub license_type: LicenseType,
    pub state: LicenseState,
    pub fee_paid_stroops: i128, // USDC stroops, as actually paid by the licensee
    pub effective_royalty_stroops: i128, // fee_paid_stroops adjusted by the dataset's QualityOracle multiplier — the basis royalty-splitter distributes from
    pub issued_ledger: u32,
    pub expiry_ledger: u32,
    pub region_code: String, // ISO 3166-1 alpha-2 or "GLOBAL"
}

/// Usage licenses by region and model class, on-chain enforcement.
#[contract]
pub struct LicenseRouter;

#[contractimpl]
impl LicenseRouter {
    pub fn initialize(env: Env, admin: Address, registry_contract: Address) {
        if env.storage().instance().has(&symbol_short!("admin")) {
            panic!("already initialized");
        }
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("admin"), &admin);
        env.storage()
            .instance()
            .set(&symbol_short!("registry"), &registry_contract);
        env.storage()
            .instance()
            .set(&symbol_short!("lic_cnt"), &0u32);
        Self::bump_instance(&env);
    }

    /// Step 1 of admin handoff: current admin proposes a successor. The
    /// proposal must be accepted by the new admin via `accept_admin` before
    /// control actually transfers — a compromised key alone can't hand
    /// itself off without the new admin's own signature.
    pub fn propose_admin(env: Env, new_admin: Address) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("proposed"), &new_admin);
    }

    /// Step 2: the proposed admin accepts, completing the handoff.
    pub fn accept_admin(env: Env) {
        let proposed: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("proposed"))
            .expect("no admin proposal pending");
        proposed.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("admin"), &proposed);
        env.storage().instance().remove(&symbol_short!("proposed"));
    }

    /// Issue a new license for a dataset. Caller pays fee_paid_stroops.
    pub fn issue_license(
        env: Env,
        licensee: Address,
        dataset_id: String,
        license_type: LicenseType,
        region_code: String,
        duration_ledgers: u32,
        fee_paid_stroops: i128,
    ) -> String {
        licensee.require_auth();
        Self::bump_instance(&env);

        // Validate fee minimums per license type
        let min_fee: i128 = match license_type {
            LicenseType::Research => 0,
            LicenseType::NonProfit => 1_000_000,    // 0.1 USDC
            LicenseType::Government => 10_000_000,  // 1 USDC
            LicenseType::Commercial => 100_000_000, // 10 USDC
        };
        if fee_paid_stroops < min_fee {
            panic!("insufficient license fee");
        }

        // Apply the dataset's QualityOracle royalty multiplier to the paid
        // fee. No oracle configured, or the cross-contract call failing for
        // any reason (dataset never attested, oracle not yet deployed,
        // etc.), degrades gracefully to 1x rather than blocking licensing.
        let multiplier: i128 = match env
            .storage()
            .instance()
            .get::<_, Address>(&symbol_short!("oracle"))
        {
            Some(oracle_contract) => {
                let oracle = QualityOracleClient::new(&env, &oracle_contract);
                match oracle.try_royalty_multiplier_bps(&dataset_id) {
                    Ok(Ok(bps)) => bps as i128,
                    _ => DEFAULT_ROYALTY_MULTIPLIER_BPS,
                }
            }
            None => DEFAULT_ROYALTY_MULTIPLIER_BPS,
        };
        let effective_royalty_stroops = fee_paid_stroops * multiplier / 10000;

        let count: u32 = env
            .storage()
            .instance()
            .get(&symbol_short!("lic_cnt"))
            .unwrap_or(0);
        let id = String::from_str(&env, &format!("lic_{}", count + 1));
        let current_ledger = env.ledger().sequence();

        let license = License {
            id: id.clone(),
            dataset_id: dataset_id.clone(),
            licensee: licensee.clone(),
            license_type,
            state: LicenseState::Active,
            fee_paid_stroops,
            effective_royalty_stroops,
            issued_ledger: current_ledger,
            expiry_ledger: current_ledger + duration_ledgers,
            region_code,
        };

        env.storage().persistent().set(&id, &license);
        env.storage()
            .instance()
            .set(&symbol_short!("lic_cnt"), &(count + 1));
        env.storage()
            .persistent()
            .extend_ttl(&id, PERSISTENT_TTL, PERSISTENT_TTL);

        env.events().publish(
            (symbol_short!("license"), symbol_short!("issued")),
            (
                id.clone(),
                dataset_id,
                licensee,
                fee_paid_stroops,
                effective_royalty_stroops,
            ),
        );

        id
    }

    /// Revoke a license (admin only).
    pub fn revoke_license(env: Env, license_id: String) {
        Self::bump_instance(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();

        let mut license: License = env
            .storage()
            .persistent()
            .get(&license_id)
            .expect("license not found");
        license.state = LicenseState::Revoked;
        env.storage().persistent().set(&license_id, &license);
        // A revoked license is still the record proving the revocation
        // happened — it must not be the one entry that quietly lapses.
        env.storage()
            .persistent()
            .extend_ttl(&license_id, PERSISTENT_TTL, PERSISTENT_TTL);

        env.events().publish(
            (symbol_short!("license"), symbol_short!("revoked")),
            license_id,
        );
    }

    /// Check if a license is currently valid.
    pub fn is_license_valid(env: Env, license_id: String) -> bool {
        let license: License = match env.storage().persistent().get(&license_id) {
            Some(l) => l,
            None => return false,
        };
        if license.state != LicenseState::Active {
            return false;
        }
        env.ledger().sequence() <= license.expiry_ledger
    }

    /// Read a license record.
    pub fn get_license(env: Env, license_id: String) -> License {
        env.storage()
            .persistent()
            .get(&license_id)
            .expect("license not found")
    }

    /// Configure (or update) the QualityOracle contract used to look up
    /// each dataset's royalty multiplier in `issue_license`. Admin only.
    pub fn set_oracle(env: Env, oracle_contract: Address) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("oracle"), &oracle_contract);
    }

    /// Refresh the contract's own instance entry on every storage-touching
    /// call. Instance storage holds the admin, the registry and oracle
    /// addresses, and the license counter, and if it lapses the whole
    /// contract is archived — every record underneath it becomes unreadable
    /// even with a perfectly fresh TTL of its own.
    fn bump_instance(env: &Env) {
        env.storage()
            .instance()
            .extend_ttl(PERSISTENT_TTL, PERSISTENT_TTL);
    }

    /// Permissionlessly extend a license's storage lifetime.
    ///
    /// Licenses are the longest-lived records in the workspace — a multi-year
    /// commercial license outlives the 90-day TTL many times over, and nothing
    /// in this contract mutates it in the meantime, so without a standalone
    /// renewal entry point a valid license simply evaporates. Renewal is open
    /// to anyone because both sides have reason to keep it alive: the licensee
    /// to prove entitlement, the dataset's contributors to prove a fee was
    /// paid and at what effective royalty.
    pub fn renew_license_ttl(env: Env, license_id: String) {
        Self::bump_instance(&env);
        if !env.storage().persistent().has(&license_id) {
            panic!("license not found");
        }
        env.storage()
            .persistent()
            .extend_ttl(&license_id, PERSISTENT_TTL, PERSISTENT_TTL);
    }

    pub fn version(_env: Env) -> u32 {
        3
    }

    pub fn upgrade(env: Env, new_wasm_hash: soroban_sdk::BytesN<32>) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();
        env.deployer().update_current_contract_wasm(new_wasm_hash.clone());
        env.events().publish(
            (symbol_short!("contract"), symbol_short!("upgraded")),
            (new_wasm_hash, env.ledger().sequence()),
        );
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod ttl_test;
