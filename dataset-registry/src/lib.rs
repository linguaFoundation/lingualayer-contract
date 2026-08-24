#![no_std]
extern crate alloc;
use alloc::format;
use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Address, Env, String, Vec};

/// How long persistent entries live before they can be archived:
/// 7,776,000 ledgers ≈ 90 days at ~1s/ledger.
const PERSISTENT_TTL: u32 = 7_776_000;

/// Total contributor shares must add up to this, in basis points.
const TOTAL_BPS: u32 = 10_000;

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DatasetState {
    Active,
    Deprecated,
    UnderReview,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ContributorShare {
    pub address: Address,
    pub share_bps: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Dataset {
    pub id: String,
    pub owner: Address,
    pub language_code: String,
    pub name: String,
    pub metadata_hash: soroban_sdk::BytesN<32>,
    pub version: u32,
    pub state: DatasetState,
    pub contributors: Vec<ContributorShare>,
    pub created_ledger: u32,
    pub sample_count: u32,
    pub duration_seconds: u32,
    pub commission_id: Option<String>, // linked commission if fulfilled
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct ContributorReputation {
    pub address: Address,
    pub reputation_score: u32, // 0-1000
    pub datasets_registered: u32,
    pub total_royalties_stroops: i128,
    pub quality_average: u32,
}

/// Dataset metadata, contributor shares, provenance, and reputation registry.
/// v3: adds sample_count, duration_seconds, commission linkage, and reputation.
///
/// # Lifecycle
///
/// Every dataset moves through an explicit state machine. `register_dataset`
/// admits a record at `Active`; from there the only legal moves are:
///
/// ```text
///   Active ──flag_dataset (admin)──▶ UnderReview
///   Active ◀──reinstate_dataset (admin)── UnderReview
///   Active ──deprecate_dataset (owner|admin)──▶ Deprecated
///   UnderReview ──deprecate_dataset (owner|admin)──▶ Deprecated
///   Deprecated ──▶ (terminal)
/// ```
///
/// `Deprecated` is terminal on purpose: downstream contracts (royalty
/// splitting, license routing) treat deprecation as a permanent signal that a
/// dataset must no longer earn, and letting it flip back to `Active` would
/// silently re-enable those flows. Mutating calls such as `update_metadata`
/// are only accepted while a dataset is `Active`, so a dataset under review
/// cannot have the very content being reviewed swapped out from under the
/// reviewer.
#[contract]
pub struct DatasetRegistry;

#[contractimpl]
impl DatasetRegistry {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&symbol_short!("admin")) {
            panic!("already initialized");
        }
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("admin"), &admin);
        env.storage().instance().set(&symbol_short!("count"), &0u32);
    }

    /// Step 1 of admin handoff: current admin proposes a successor. The
    /// proposal must be accepted by the new admin via `accept_admin` before
    /// control actually transfers — a compromised key alone can't hand
    /// itself off without the new admin's own signature.
    pub fn propose_admin(env: Env, new_admin: Address) {
        let admin = Self::admin(&env);
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

    /// Read the configured admin, panicking if the contract was never
    /// initialized. Every admin-gated entry point goes through here so an
    /// uninitialized contract fails with one consistent message instead of
    /// falling through to an unauthorized state.
    fn admin(env: &Env) -> Address {
        env.storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized")
    }

    pub fn register_dataset(
        env: Env,
        owner: Address,
        language_code: String,
        name: String,
        metadata_hash: soroban_sdk::BytesN<32>,
        contributors: Vec<ContributorShare>,
        sample_count: u32,
        duration_seconds: u32,
        commission_id: Option<String>,
    ) -> String {
        owner.require_auth();

        // Registration is admin-gated at the contract level only in the sense
        // that the registry must exist; refusing here keeps `count` and the
        // admin key in a consistent initialized state.
        let _ = Self::admin(&env);

        if metadata_hash == soroban_sdk::BytesN::from_array(&env, &[0u8; 32]) {
            panic!("metadata hash cannot be zero");
        }

        // Reject a dataset already registered under this exact metadata
        // hash — otherwise the same dataset could be re-registered under a
        // fresh id to farm registration reputation or double-collect
        // commission-fulfilment credit for one piece of underlying work.
        let hash_key = String::from_str(&env, &format!("hash_{:?}", metadata_hash));
        if env.storage().persistent().has(&hash_key) {
            panic!("dataset with this metadata hash is already registered");
        }

        Self::validate_shares(&contributors);

        let count: u32 = env
            .storage()
            .instance()
            .get(&symbol_short!("count"))
            .unwrap_or(0);
        let id = String::from_str(&env, &format!("ds_{}", count + 1));

        let dataset = Dataset {
            id: id.clone(),
            owner: owner.clone(),
            language_code: language_code.clone(),
            name: name.clone(),
            metadata_hash,
            version: 1,
            state: DatasetState::Active,
            contributors,
            created_ledger: env.ledger().sequence(),
            sample_count,
            duration_seconds,
            commission_id,
        };

        env.storage().persistent().set(&id, &dataset);
        env.storage().persistent().set(&hash_key, &id);
        env.storage()
            .instance()
            .set(&symbol_short!("count"), &(count + 1));
        env.storage()
            .persistent()
            .extend_ttl(&id, PERSISTENT_TTL, PERSISTENT_TTL);
        env.storage()
            .persistent()
            .extend_ttl(&hash_key, PERSISTENT_TTL, PERSISTENT_TTL);

        // Update owner reputation
        Self::increment_reputation(&env, &owner);

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("created")),
            (id.clone(), owner, language_code, sample_count),
        );
        id
    }

    /// Contributor shares are the input the royalty splitter trusts, so they
    /// are validated strictly rather than merely summed: an empty set, a
    /// zero-weight entry, or a duplicated address would each produce a split
    /// that silently misallocates funds later.
    fn validate_shares(contributors: &Vec<ContributorShare>) {
        if contributors.is_empty() {
            panic!("dataset must have at least one contributor");
        }

        let mut total: u32 = 0;
        for (i, c) in contributors.iter().enumerate() {
            if c.share_bps == 0 {
                panic!("contributor share must be greater than zero");
            }
            // A duplicated address would pass the 10000 bps check while
            // concentrating payout in one party under two entries, so reject
            // it outright. Contributor lists are small (bounded by the 10000
            // bps budget and a non-zero minimum), so the pairwise scan is
            // cheap in practice.
            for (j, other) in contributors.iter().enumerate() {
                if i < j && c.address == other.address {
                    panic!("duplicate contributor address");
                }
            }
            // Checked so an oversized list can never wrap past 10000 and pass
            // validation with an absurd allocation.
            total = total
                .checked_add(c.share_bps)
                .expect("contributor shares overflow");
        }

        if total != TOTAL_BPS {
            panic!("contributor shares must sum to 10000 bps");
        }
    }

    fn increment_reputation(env: &Env, address: &Address) {
        let rep_key = String::from_str(env, &format!("rep_{:?}", address));
        let mut rep: ContributorReputation =
            env.storage()
                .persistent()
                .get(&rep_key)
                .unwrap_or(ContributorReputation {
                    address: address.clone(),
                    reputation_score: 0,
                    datasets_registered: 0,
                    total_royalties_stroops: 0,
                    quality_average: 0,
                });
        rep.datasets_registered += 1;
        rep.reputation_score = (rep.reputation_score + 50).min(1000);
        env.storage().persistent().set(&rep_key, &rep);
        env.storage()
            .persistent()
            .extend_ttl(&rep_key, PERSISTENT_TTL, PERSISTENT_TTL);
    }

    pub fn get_reputation(env: Env, address: Address) -> ContributorReputation {
        let rep_key = String::from_str(&env, &format!("rep_{:?}", address));
        env.storage()
            .persistent()
            .get(&rep_key)
            .expect("no reputation data")
    }

    pub fn get_dataset(env: Env, dataset_id: String) -> Dataset {
        env.storage()
            .persistent()
            .get(&dataset_id)
            .expect("dataset not found")
    }

    pub fn dataset_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("count"))
            .unwrap_or(0)
    }

    /// Look up which dataset (if any) already owns a given metadata hash —
    /// lets a caller check for a duplicate before attempting registration
    /// instead of relying solely on register_dataset's panic.
    pub fn dataset_id_for_hash(env: Env, metadata_hash: soroban_sdk::BytesN<32>) -> Option<String> {
        let hash_key = String::from_str(&env, &format!("hash_{:?}", metadata_hash));
        env.storage().persistent().get(&hash_key)
    }

    /// Current lifecycle state of a dataset.
    pub fn get_state(env: Env, dataset_id: String) -> DatasetState {
        Self::load(&env, &dataset_id).state
    }

    pub fn update_metadata(env: Env, dataset_id: String, new_hash: soroban_sdk::BytesN<32>) {
        let mut ds = Self::load(&env, &dataset_id);
        ds.owner.require_auth();

        // Content may only change while the dataset is Active. Allowing it
        // under review would let an owner swap out the very bytes a reviewer
        // is evaluating; allowing it after deprecation would resurrect a
        // record downstream contracts have already written off.
        if ds.state != DatasetState::Active {
            panic!("dataset must be active to update metadata");
        }

        if new_hash == soroban_sdk::BytesN::from_array(&env, &[0u8; 32]) {
            panic!("metadata hash cannot be zero");
        }
        if new_hash == ds.metadata_hash {
            panic!("metadata hash unchanged");
        }

        // Move the hash index with the dataset so `dataset_id_for_hash` stays
        // truthful and the old hash is freed for a genuinely different
        // dataset, while the new hash still can't collide with another entry.
        let new_key = String::from_str(&env, &format!("hash_{:?}", new_hash));
        if env.storage().persistent().has(&new_key) {
            panic!("dataset with this metadata hash is already registered");
        }
        let old_key = String::from_str(&env, &format!("hash_{:?}", ds.metadata_hash));
        env.storage().persistent().remove(&old_key);
        env.storage().persistent().set(&new_key, &dataset_id);
        env.storage()
            .persistent()
            .extend_ttl(&new_key, PERSISTENT_TTL, PERSISTENT_TTL);

        ds.metadata_hash = new_hash.clone();
        ds.version += 1;
        let version = ds.version;
        Self::save(&env, &dataset_id, &ds);

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("updated")),
            (dataset_id, new_hash, version),
        );
    }

    /// Admin flags an Active dataset for review — a reversible hold that
    /// freezes metadata updates without permanently retiring the record.
    pub fn flag_dataset(env: Env, dataset_id: String) {
        let admin = Self::admin(&env);
        admin.require_auth();

        let mut ds = Self::load(&env, &dataset_id);
        if ds.state != DatasetState::Active {
            panic!("only an active dataset can be flagged for review");
        }
        ds.state = DatasetState::UnderReview;
        Self::save(&env, &dataset_id, &ds);

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("flagged")),
            dataset_id,
        );
    }

    /// Admin clears a review, returning the dataset to Active.
    pub fn reinstate_dataset(env: Env, dataset_id: String) {
        let admin = Self::admin(&env);
        admin.require_auth();

        let mut ds = Self::load(&env, &dataset_id);
        if ds.state != DatasetState::UnderReview {
            panic!("only a dataset under review can be reinstated");
        }
        ds.state = DatasetState::Active;
        Self::save(&env, &dataset_id, &ds);

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("reinstate")),
            dataset_id,
        );
    }

    /// Retire a dataset permanently. Either the dataset owner or the protocol
    /// admin may do this, so `caller` is explicit: Soroban auth cannot be
    /// probed conditionally, so the caller declares which identity it is
    /// acting as and that identity both signs and is checked against the two
    /// permitted roles.
    pub fn deprecate_dataset(env: Env, dataset_id: String, caller: Address) {
        caller.require_auth();

        let mut ds = Self::load(&env, &dataset_id);
        let admin = Self::admin(&env);
        if caller != ds.owner && caller != admin {
            panic!("only the dataset owner or admin can deprecate");
        }

        // Terminal state — re-deprecating would emit a second event and let
        // downstream listeners double-count a retirement that already happened.
        if ds.state == DatasetState::Deprecated {
            panic!("dataset is already deprecated");
        }

        ds.state = DatasetState::Deprecated;
        Self::save(&env, &dataset_id, &ds);

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("deprecate")),
            dataset_id,
        );
    }

    fn load(env: &Env, dataset_id: &String) -> Dataset {
        env.storage()
            .persistent()
            .get(dataset_id)
            .expect("dataset not found")
    }

    /// Write a dataset back and refresh its TTL in one place, so no mutating
    /// path can persist a record and then let it expire early.
    fn save(env: &Env, dataset_id: &String, ds: &Dataset) {
        env.storage().persistent().set(dataset_id, ds);
        env.storage()
            .persistent()
            .extend_ttl(dataset_id, PERSISTENT_TTL, PERSISTENT_TTL);
    }

    pub fn version(_env: Env) -> u32 {
        3
    }
}

#[cfg(test)]
mod test;
