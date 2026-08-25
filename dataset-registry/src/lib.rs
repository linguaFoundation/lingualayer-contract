#![no_std]
#![allow(clippy::too_many_arguments)]
extern crate alloc;
use alloc::format;
use soroban_sdk::{contract, contractimpl, contracttype, contracterror, symbol_short, Address, Env, String, Vec};

/// How long persistent entries live before they can be archived:
/// 7,776,000 ledgers ≈ 90 days at ~1s/ledger.
const PERSISTENT_TTL: u32 = 7_776_000;

/// Total contributor shares must add up to this, in basis points.
const TOTAL_BPS: u32 = 10_000;


#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NoAdminProposalPending = 3,
    MetadataHashCannotBeZero = 4,
    DatasetWithThisMetadataHashIsAlreadyRegistered = 5,
    DatasetMustHaveAtLeastOneContributor = 6,
    ContributorShareMustBeGreaterThanZero = 7,
    DuplicateContributorAddress = 8,
    ContributorSharesOverflow = 9,
    ContributorSharesMustSumTo10000 = 10,
    NoReputationData = 11,
    DatasetNotFound = 12,
    DatasetMustBeActiveToUpdateMetadata = 13,
    MetadataHashUnchanged = 14,
    OnlyAnActiveDatasetCanBeFlaggedForReview = 15,
    OnlyADatasetUnderReviewCanBeReinstated = 16,
    OnlyTheDatasetOwnerOrAdminCanDeprecate = 17,
    DatasetIsAlreadyDeprecated = 18,
}

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
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&symbol_short!("admin")) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("admin"), &admin);
        env.storage().instance().set(&symbol_short!("count"), &0u32);
        Self::bump_instance(&env);
        Ok(())
    }

    /// Step 1 of admin handoff: current admin proposes a successor. The
    /// proposal must be accepted by the new admin via `accept_admin` before
    /// control actually transfers — a compromised key alone can't hand
    /// itself off without the new admin's own signature.
    pub fn propose_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        let admin = Self::admin(&env)?;
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("proposed"), &new_admin);
        Ok(())
    }

    /// Step 2: the proposed admin accepts, completing the handoff.
    pub fn accept_admin(env: Env) -> Result<(), Error> {
        let proposed: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("proposed"))
            .ok_or(Error::NoAdminProposalPending)?;
        proposed.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("admin"), &proposed);
        env.storage().instance().remove(&symbol_short!("proposed"));
        Ok(())
    }

    /// Read the configured admin, panicking if the contract was never
    /// initialized. Every admin-gated entry point goes through here so an
    /// uninitialized contract fails with one consistent message instead of
    /// falling through to an unauthorized state.
    fn admin(env: &Env) -> Result<Address, Error> {
        env.storage()
            .instance()
            .get(&symbol_short!("admin"))
            .ok_or(Error::NotInitialized)?
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
    ) -> Result<String, Error> {
        owner.require_auth();

        // Registration is admin-gated at the contract level only in the sense
        // that the registry must exist; refusing here keeps `count` and the
        // admin key in a consistent initialized state.
        let _ = Self::admin(&env)?;

        if metadata_hash == soroban_sdk::BytesN::from_array(&env, &[0u8; 32]) {
            return Err(Error::MetadataHashCannotBeZero);
        }

        // Reject a dataset already registered under this exact metadata
        // hash — otherwise the same dataset could be re-registered under a
        // fresh id to farm registration reputation or double-collect
        // commission-fulfilment credit for one piece of underlying work.
        let hash_key = String::from_str(&env, &format!("hash_{:?}", metadata_hash));
        if env.storage().persistent().has(&hash_key) {
            return Err(Error::DatasetWithThisMetadataHashIsAlreadyRegistered);
        }

        Self::validate_shares(&contributors)?;

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

        Self::bump_instance(&env);
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
        Self::increment_reputation(&env, &owner)?;

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("created")),
            (id.clone(), owner, language_code, sample_count),
        );
        Ok(id)
    }

    /// Contributor shares are the input the royalty splitter trusts, so they
    /// are validated strictly rather than merely summed: an empty set, a
    /// zero-weight entry, or a duplicated address would each produce a split
    /// that silently misallocates funds later.
    fn validate_shares(contributors: &Vec<ContributorShare>) -> Result<(), Error> {
        if contributors.is_empty() {
            return Err(Error::DatasetMustHaveAtLeastOneContributor);
        }

        let mut total: u32 = 0;
        for (i, c) in contributors.iter().enumerate() {
            if c.share_bps == 0 {
                return Err(Error::ContributorShareMustBeGreaterThanZero);
            }
            // A duplicated address would pass the 10000 bps check while
            // concentrating payout in one party under two entries, so reject
            // it outright. Contributor lists are small (bounded by the 10000
            // bps budget and a non-zero minimum), so the pairwise scan is
            // cheap in practice.
            for (j, other) in contributors.iter().enumerate() {
                if i < j && c.address == other.address {
                    return Err(Error::DuplicateContributorAddress);
                }
            }
            // Checked so an oversized list can never wrap past 10000 and pass
            // validation with an absurd allocation.
            total = total
                .checked_add(c.share_bps)
                .ok_or(Error::ContributorSharesOverflow)?;
        }

        if total != TOTAL_BPS {
            return Err(Error::ContributorSharesMustSumTo10000);
        }
        Ok(())
    }

    fn increment_reputation(env: &Env, address: &Address) -> Result<(), Error> {
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
        Ok(())
    }

    pub fn get_reputation(env: Env, address: Address) -> Result<ContributorReputation, Error> {
        let rep_key = String::from_str(&env, &format!("rep_{:?}", address));
        env.storage()
            .persistent()
            .get(&rep_key)
            .ok_or(Error::NoReputationData)?
    }

    pub fn get_dataset(env: Env, dataset_id: String) -> Result<Dataset, Error> {
        env.storage()
            .persistent()
            .get(&dataset_id)
            .ok_or(Error::DatasetNotFound)?
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
    pub fn get_state(env: Env, dataset_id: String) -> Result<DatasetState, Error> {
        Ok(Self::load(&env, &dataset_id)?.state)
    }

    pub fn update_metadata(env: Env, dataset_id: String, new_hash: soroban_sdk::BytesN<32>) -> Result<(), Error> {
        let mut ds = Self::load(&env, &dataset_id)?;
        ds.owner.require_auth();

        // Content may only change while the dataset is Active. Allowing it
        // under review would let an owner swap out the very bytes a reviewer
        // is evaluating; allowing it after deprecation would resurrect a
        // record downstream contracts have already written off.
        if ds.state != DatasetState::Active {
            return Err(Error::DatasetMustBeActiveToUpdateMetadata);
        }

        if new_hash == soroban_sdk::BytesN::from_array(&env, &[0u8; 32]) {
            return Err(Error::MetadataHashCannotBeZero);
        }
        if new_hash == ds.metadata_hash {
            return Err(Error::MetadataHashUnchanged);
        }

        // Move the hash index with the dataset so `dataset_id_for_hash` stays
        // truthful and the old hash is freed for a genuinely different
        // dataset, while the new hash still can't collide with another entry.
        let new_key = String::from_str(&env, &format!("hash_{:?}", new_hash));
        if env.storage().persistent().has(&new_key) {
            return Err(Error::DatasetWithThisMetadataHashIsAlreadyRegistered);
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
        Ok(())
    }


    /// Admin flags an Active dataset for review — a reversible hold that
    /// freezes metadata updates without permanently retiring the record.
    pub fn flag_dataset(env: Env, dataset_id: String) -> Result<(), Error> {
        let admin = Self::admin(&env)?;
        admin.require_auth();

        let mut ds = Self::load(&env, &dataset_id)?;
        if ds.state != DatasetState::Active {
            return Err(Error::OnlyAnActiveDatasetCanBeFlaggedForReview);
        }
        ds.state = DatasetState::UnderReview;
        Self::save(&env, &dataset_id, &ds);

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("flagged")),
            dataset_id,
        );
        Ok(())
    }

    /// Admin clears a review, returning the dataset to Active.
    pub fn reinstate_dataset(env: Env, dataset_id: String) -> Result<(), Error> {
        let admin = Self::admin(&env)?;
        admin.require_auth();

        let mut ds = Self::load(&env, &dataset_id)?;
        if ds.state != DatasetState::UnderReview {
            return Err(Error::OnlyADatasetUnderReviewCanBeReinstated);
        }
        ds.state = DatasetState::Active;
        Self::save(&env, &dataset_id, &ds);

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("reinstate")),
            dataset_id,
        );
        Ok(())
    }

    /// Retire a dataset permanently. Either the dataset owner or the protocol
    /// admin may do this, so `caller` is explicit: Soroban auth cannot be
    /// probed conditionally, so the caller declares which identity it is
    /// acting as and that identity both signs and is checked against the two
    /// permitted roles.
    pub fn deprecate_dataset(env: Env, dataset_id: String, caller: Address) -> Result<(), Error> {
        caller.require_auth();

        let mut ds = Self::load(&env, &dataset_id)?;
        let admin = Self::admin(&env)?;
        if caller != ds.owner && caller != admin {
            return Err(Error::OnlyTheDatasetOwnerOrAdminCanDeprecate);
        }

        // Terminal state — re-deprecating would emit a second event and let
        // downstream listeners double-count a retirement that already happened.
        if ds.state == DatasetState::Deprecated {
            return Err(Error::DatasetIsAlreadyDeprecated);
        }

        ds.state = DatasetState::Deprecated;
        Self::save(&env, &dataset_id, &ds);

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("deprecate")),
            dataset_id,
        );
        Ok(())
    }

    /// Permissionlessly extend a dataset's storage lifetime.
    ///
    /// No auth: a dataset lapsing is a loss to every contributor and licensee
    /// downstream, not just its owner, so anyone willing to pay the fee may
    /// keep it alive. There is nothing to gain by calling this on someone
    /// else's record beyond footing the bill for it.
    ///
    /// Renews the hash index alongside the record itself — letting
    /// `hash_{metadata_hash}` expire while the dataset survives would leave
    /// `dataset_id_for_hash` silently returning None for a live dataset, and
    /// free its hash for re-registration by an unrelated one.
    pub fn renew_dataset_ttl(env: Env, dataset_id: String) -> Result<(), Error> {
        Self::bump_instance(&env);
        let ds: Dataset = env
            .storage()
            .persistent()
            .get(&dataset_id)
            .ok_or(Error::DatasetNotFound)?;

        env.storage()
            .persistent()
            .extend_ttl(&dataset_id, PERSISTENT_TTL, PERSISTENT_TTL);

        let hash_key = String::from_str(&env, &format!("hash_{:?}", ds.metadata_hash));
        if env.storage().persistent().has(&hash_key) {
            env.storage()
                .persistent()
                .extend_ttl(&hash_key, PERSISTENT_TTL, PERSISTENT_TTL);
        }
        Ok(())
    }

    /// Permissionlessly extend a contributor's reputation entry. Reputation
    /// only accrues on registration, so a prolific early contributor who then
    /// goes quiet is exactly the account whose history would otherwise lapse.
    pub fn renew_reputation_ttl(env: Env, address: Address) -> Result<(), Error> {
        Self::bump_instance(&env);
        let rep_key = String::from_str(&env, &format!("rep_{:?}", address));
        if !env.storage().persistent().has(&rep_key) {
            return Err(Error::NoReputationData);
        }
        env.storage()
            .persistent()
            .extend_ttl(&rep_key, PERSISTENT_TTL, PERSISTENT_TTL);
        Ok(())
    }

    /// Refresh the contract's own instance entry (admin, dataset counter) on
    /// every storage-touching call.
    ///
    /// This is the failure mode that outranks every per-record TTL: instance
    /// storage holds the admin and the id counter, and if it lapses the whole
    /// contract is archived — every dataset underneath it becomes unreadable
    /// even with a perfectly fresh TTL of its own. Renewing it here means any
    /// call that keeps a record alive keeps the contract alive too.
    fn bump_instance(env: &Env) {
        env.storage()
            .instance()
            .extend_ttl(PERSISTENT_TTL, PERSISTENT_TTL);
    }

    fn load(env: &Env, dataset_id: &String) -> Result<Dataset, Error> {
        env.storage()
            .persistent()
            .get(dataset_id)
            .ok_or(Error::DatasetNotFound)?
    }

    /// Write a dataset back and refresh its TTL in one place, so no mutating
    /// path can persist a record and then let it expire early.
    fn save(env: &Env, dataset_id: &String, ds: &Dataset) {
        Self::bump_instance(env);
        env.storage().persistent().set(dataset_id, ds);
        env.storage()
            .persistent()
            .extend_ttl(dataset_id, PERSISTENT_TTL, PERSISTENT_TTL);
    }

    pub fn version(_env: Env) -> u32 {
        3
    }

    pub fn upgrade(env: Env, new_wasm_hash: soroban_sdk::BytesN<32>) {
        let admin = Self::admin(&env).expect("not initialized");
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
