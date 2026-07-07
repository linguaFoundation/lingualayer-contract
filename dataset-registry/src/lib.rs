#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short,
    Address, Env, String, Vec,
};

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DatasetState { Active, Deprecated, UnderReview }

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
#[contract]
pub struct DatasetRegistry;

#[contractimpl]
impl DatasetRegistry {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&symbol_short!("admin")) {
            panic!("already initialized");
        }
        admin.require_auth();
        env.storage().instance().set(&symbol_short!("admin"), &admin);
        env.storage().instance().set(&symbol_short!("count"), &0u32);
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

        let total: u32 = contributors.iter().map(|c| c.share_bps).sum();
        if total != 10000 { panic!("contributor shares must sum to 10000 bps"); }

        let count: u32 = env.storage().instance()
            .get(&symbol_short!("count")).unwrap_or(0);
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
        env.storage().instance().set(&symbol_short!("count"), &(count + 1));
        env.storage().persistent().extend_ttl(&id, 7_776_000, 7_776_000);

        // Update owner reputation
        Self::increment_reputation(&env, &owner);

        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("created")),
            (id.clone(), owner, language_code, sample_count),
        );
        id
    }

    fn increment_reputation(env: &Env, address: &Address) {
        let rep_key = String::from_str(env, &format!("rep_{}", address));
        let mut rep: ContributorReputation = env.storage().persistent()
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
        env.storage().persistent().extend_ttl(&rep_key, 7_776_000, 7_776_000);
    }

    pub fn get_reputation(env: Env, address: Address) -> ContributorReputation {
        let rep_key = String::from_str(&env, &format!("rep_{}", address));
        env.storage().persistent()
            .get(&rep_key)
            .expect("no reputation data")
    }

    pub fn get_dataset(env: Env, dataset_id: String) -> Dataset {
        env.storage().persistent().get(&dataset_id).expect("dataset not found")
    }

    pub fn dataset_count(env: Env) -> u32 {
        env.storage().instance().get(&symbol_short!("count")).unwrap_or(0)
    }

    pub fn update_metadata(env: Env, dataset_id: String, new_hash: soroban_sdk::BytesN<32>) {
        let mut ds: Dataset = env.storage().persistent()
            .get(&dataset_id).expect("dataset not found");
        ds.owner.require_auth();
        ds.metadata_hash = new_hash;
        ds.version += 1;
        env.storage().persistent().set(&dataset_id, &ds);
    }

    pub fn deprecate_dataset(env: Env, dataset_id: String) {
        let mut ds: Dataset = env.storage().persistent()
            .get(&dataset_id).expect("dataset not found");
        ds.owner.require_auth();
        ds.state = DatasetState::Deprecated;
        env.storage().persistent().set(&dataset_id, &ds);
        env.events().publish(
            (symbol_short!("dataset"), symbol_short!("deprecated")),
            dataset_id,
        );
    }

    pub fn version(_env: Env) -> u32 { 3 }
}
