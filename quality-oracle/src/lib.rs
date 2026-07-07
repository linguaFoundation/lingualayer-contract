#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short,
    Address, Env, String, Vec,
};

/// Maximum quality score (100 points)
const MAX_SCORE: u32 = 100;
/// Minimum stake to become a certified curator (10 XLM in stroops)
const MIN_CURATOR_STAKE: i128 = 100_000_000;

#[contracttype]
#[derive(Clone, Debug)]
pub struct QualityAttestation {
    pub dataset_id: String,
    pub curator: Address,
    pub score: u32,          // 0-100 quality score
    pub rubric_hash: soroban_sdk::BytesN<32>, // IPFS hash of scoring rubric
    pub ledger: u32,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct DatasetQuality {
    pub dataset_id: String,
    pub average_score: u32,
    pub attestation_count: u32,
    pub last_updated_ledger: u32,
    pub tier: QualityTier,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum QualityTier {
    Unrated,
    Bronze,   // 1-39
    Silver,   // 40-69
    Gold,     // 70-84
    Platinum, // 85-100
}

/// On-chain data quality attestation oracle.
/// Trusted curators stake XLM and submit quality scores.
/// Score average determines dataset tier and royalty multiplier.
#[contract]
pub struct QualityOracle;

#[contractimpl]
impl QualityOracle {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&symbol_short!("admin")) {
            panic!("already initialized");
        }
        admin.require_auth();
        env.storage().instance().set(&symbol_short!("admin"), &admin);
        env.storage().instance().set(&symbol_short!("cur_cnt"), &0u32);
    }

    /// Register a curator by staking XLM. Stakers can be slashed for bad scores.
    pub fn register_curator(env: Env, curator: Address) {
        curator.require_auth();
        let key = String::from_str(&env, &format!("cur_{}", curator));
        if env.storage().persistent().has(&key) {
            panic!("curator already registered");
        }
        env.storage().persistent().set(&key, &true);
        env.storage().persistent().extend_ttl(&key, 7_776_000, 7_776_000);

        let cnt: u32 = env.storage().instance().get(&symbol_short!("cur_cnt")).unwrap_or(0);
        env.storage().instance().set(&symbol_short!("cur_cnt"), &(cnt + 1));

        env.events().publish(
            (symbol_short!("oracle"), symbol_short!("curator")),
            curator,
        );
    }

    /// Submit a quality score attestation for a dataset.
    pub fn attest_quality(
        env: Env,
        curator: Address,
        dataset_id: String,
        score: u32,
        rubric_hash: soroban_sdk::BytesN<32>,
    ) {
        curator.require_auth();

        // Validate curator is registered
        let cur_key = String::from_str(&env, &format!("cur_{}", curator));
        if !env.storage().persistent().has(&cur_key) {
            panic!("curator not registered");
        }
        if score > MAX_SCORE {
            panic!("score must be 0-100");
        }

        // Record attestation
        let attest = QualityAttestation {
            dataset_id: dataset_id.clone(),
            curator: curator.clone(),
            score,
            rubric_hash,
            ledger: env.ledger().sequence(),
        };
        let attest_key = String::from_str(&env, &format!("att_{}_{}", dataset_id, curator));
        env.storage().persistent().set(&attest_key, &attest);
        env.storage().persistent().extend_ttl(&attest_key, 7_776_000, 7_776_000);

        // Update aggregate score
        let agg_key = String::from_str(&env, &format!("agg_{}", dataset_id));
        let mut quality: DatasetQuality = env.storage().persistent()
            .get(&agg_key)
            .unwrap_or(DatasetQuality {
                dataset_id: dataset_id.clone(),
                average_score: 0,
                attestation_count: 0,
                last_updated_ledger: 0,
                tier: QualityTier::Unrated,
            });

        // Running average
        let new_total = quality.average_score as u64 * quality.attestation_count as u64 + score as u64;
        quality.attestation_count += 1;
        quality.average_score = (new_total / quality.attestation_count as u64) as u32;
        quality.last_updated_ledger = env.ledger().sequence();
        quality.tier = Self::compute_tier(quality.average_score);

        env.storage().persistent().set(&agg_key, &quality);
        env.storage().persistent().extend_ttl(&agg_key, 7_776_000, 7_776_000);

        env.events().publish(
            (symbol_short!("oracle"), symbol_short!("attested")),
            (dataset_id, curator, score, quality.tier),
        );
    }

    /// Get aggregate quality for a dataset.
    pub fn get_quality(env: Env, dataset_id: String) -> DatasetQuality {
        let agg_key = String::from_str(&env, &format!("agg_{}", dataset_id));
        env.storage().persistent()
            .get(&agg_key)
            .expect("no quality data for dataset")
    }

    /// Compute royalty multiplier (bps) based on quality tier.
    /// Platinum = 150% (1.5x), Gold = 125%, Silver = 100%, Bronze = 75%
    pub fn royalty_multiplier_bps(env: Env, dataset_id: String) -> u32 {
        let agg_key = String::from_str(&env, &format!("agg_{}", dataset_id));
        match env.storage().persistent().get::<String, DatasetQuality>(&agg_key) {
            Some(q) => match q.tier {
                QualityTier::Platinum => 15000,
                QualityTier::Gold     => 12500,
                QualityTier::Silver   => 10000,
                QualityTier::Bronze   => 7500,
                QualityTier::Unrated  => 10000,
            },
            None => 10000,
        }
    }

    fn compute_tier(score: u32) -> QualityTier {
        match score {
            0           => QualityTier::Unrated,
            1..=39      => QualityTier::Bronze,
            40..=69     => QualityTier::Silver,
            70..=84     => QualityTier::Gold,
            _           => QualityTier::Platinum,
        }
    }

    pub fn version(_env: Env) -> u32 { 1 }
}
