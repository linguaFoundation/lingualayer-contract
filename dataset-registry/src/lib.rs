#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short,
    Address, Env, Map, String, Vec,
};

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
    pub share_bps: u32, // basis points (0-10000)
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Dataset {
    pub id: String,
    pub owner: Address,
    pub language_code: String, // ISO 639-3 (e.g. "yor", "hau", "ibo")
    pub name: String,
    pub metadata_hash: soroban_sdk::BytesN<32>, // IPFS CIDv1 hash
    pub version: u32,
    pub state: DatasetState,
    pub contributors: Vec<ContributorShare>,
    pub created_ledger: u32,
}

const ADMIN_KEY: &str = "admin";
const DATASET_PREFIX: &str = "ds";
const DATASET_COUNT: &str = "count";

/// Dataset metadata, contributor shares, and provenance registry.
#[contract]
pub struct DatasetRegistry;

#[contractimpl]
impl DatasetRegistry {
    /// One-time initialization — sets protocol admin.
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&symbol_short!("admin")) {
            panic!("already initialized");
        }
        admin.require_auth();
        env.storage().instance().set(&symbol_short!("admin"), &admin);
        env.storage().instance().set(&symbol_short!("count"), &0u32);
        env.events().publish(
            (symbol_short!("init"), symbol_short!("admin")),
            admin,
        );
    }

    /// Register a new language dataset on-chain.
    pub fn register_dataset(
        env: Env,
        owner: Address,
        language_code: String,
        name: String,
        metadata_hash: soroban_sdk::BytesN<32>,
        contributors: Vec<ContributorShare>,
    ) -> String {
        owner.require_auth();

        // Validate contributor shares sum to 10000 bps (100%)
        let total: u32 = contributors.iter().map(|c| c.share_bps).sum();
        if total != 10000 {
            panic!("contributor shares must sum to 10000 bps");
        }

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
            metadata_hash: metadata_hash.clone(),
            version: 1,
            state: DatasetState::Active,
            contributors,
            created_ledger: env.ledger().sequence(),
        };

        env.storage().persistent().set(&id, &dataset);
        env.storage()
            .instance()
            .set(&symbol_short!("count"), &(count + 1));

        // Extend TTL for persistent storage (90 days of ledgers ~7,776,000)
        env.storage()
            .persistent()
            .extend_ttl(&id, 7_776_000, 7_776_000);

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("created")),
            (id.clone(), owner, language_code),
        );

        id
    }

    /// Update dataset metadata (owner only). Increments version.
    pub fn update_metadata(
        env: Env,
        dataset_id: String,
        new_metadata_hash: soroban_sdk::BytesN<32>,
    ) {
        let mut dataset: Dataset = env
            .storage()
            .persistent()
            .get(&dataset_id)
            .expect("dataset not found");
        dataset.owner.require_auth();
        dataset.metadata_hash = new_metadata_hash.clone();
        dataset.version += 1;
        env.storage().persistent().set(&dataset_id, &dataset);
        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("updated")),
            (dataset_id, new_metadata_hash, dataset.version),
        );
    }

    /// Deprecate a dataset (admin or owner).
    pub fn deprecate_dataset(env: Env, dataset_id: String) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        let mut dataset: Dataset = env
            .storage()
            .persistent()
            .get(&dataset_id)
            .expect("dataset not found");

        // Either admin or owner can deprecate
        let caller_is_admin = env.current_contract_address() == admin.clone();
        if !caller_is_admin {
            dataset.owner.require_auth();
        }

        dataset.state = DatasetState::Deprecated;
        env.storage().persistent().set(&dataset_id, &dataset);
        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("deprecated")),
            dataset_id,
        );
    }

    /// Read a dataset record.
    pub fn get_dataset(env: Env, dataset_id: String) -> Dataset {
        env.storage()
            .persistent()
            .get(&dataset_id)
            .expect("dataset not found")
    }

    /// Total number of registered datasets.
    pub fn dataset_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("count"))
            .unwrap_or(0)
    }

    /// Contract version marker.
    pub fn version(_env: Env) -> u32 {
        2
    }
}
