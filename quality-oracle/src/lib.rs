#![no_std]
extern crate alloc;
use alloc::format;
use soroban_sdk::{contract, contractimpl, contracttype, symbol_short, Address, Env, String, Vec};

/// Maximum quality score (100 points)
const MAX_SCORE: u32 = 100;
/// Minimum stake to become a certified curator (10 XLM in stroops)
const MIN_CURATOR_STAKE: i128 = 100_000_000;
/// A curator's score must deviate from consensus by more than this many
/// points before it's considered a malicious/outlier attestation.
const SLASH_DEVIATION_THRESHOLD: u32 = 30;
/// Fraction of stake burned per slash (20%).
const SLASH_BPS: i128 = 2_000;
const TOTAL_BPS: i128 = 10_000;

#[contracttype]
#[derive(Clone, Debug)]
pub struct QualityAttestation {
    pub dataset_id: String,
    pub curator: Address,
    pub score: u32,                           // 0-100 quality score
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

/// A curator's standing after zero or more slashes.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CuratorStatus {
    Active,
    SlashWarning,
    Banned,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct CuratorState {
    pub stake: i128,
    pub status: CuratorStatus,
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
        env.storage()
            .instance()
            .set(&symbol_short!("admin"), &admin);
        env.storage()
            .instance()
            .set(&symbol_short!("cur_cnt"), &0u32);
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

    /// Freeze every state-mutating entry point. Admin only.
    ///
    /// This is incident-response machinery: if a vulnerability is found before
    /// or during testnet, the damage window is however long it takes to get a
    /// transaction through, not however long it takes to ship a fix.
    ///
    /// Reads stay available while paused, deliberately. Integrators and the
    /// front end need to keep answering questions about existing state during
    /// an incident, and a read cannot make the problem worse.
    pub fn pause(env: Env) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();

        if Self::is_paused(env.clone()) {
            panic!("already paused");
        }
        env.storage()
            .instance()
            .set(&symbol_short!("paused"), &true);

        env.events().publish(
            (symbol_short!("pause"), symbol_short!("paused")),
            (admin, env.ledger().timestamp()),
        );
    }

    /// Lift the freeze. Admin only.
    pub fn unpause(env: Env) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();

        if !Self::is_paused(env.clone()) {
            panic!("not paused");
        }
        env.storage()
            .instance()
            .set(&symbol_short!("paused"), &false);

        env.events().publish(
            (symbol_short!("pause"), symbol_short!("unpaused")),
            (admin, env.ledger().timestamp()),
        );
    }

    /// Whether writes are currently frozen. A read, so it answers while paused.
    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&symbol_short!("paused"))
            .unwrap_or(false)
    }

    /// Reject a state-mutating call while the contract is frozen.
    ///
    /// Checked before authorization on purpose: a paused contract rejects the
    /// call whoever is making it, so there is no reason to do the more
    /// expensive auth work first, and no signature is consumed by a call that
    /// was never going to land.
    fn require_not_paused(env: &Env) {
        if env
            .storage()
            .instance()
            .get(&symbol_short!("paused"))
            .unwrap_or(false)
        {
            panic!("contract paused");
        }
    }

    /// Register a curator by staking XLM. Stakers can be slashed for bad scores.
    pub fn register_curator(env: Env, curator: Address) {
        Self::require_not_paused(&env);
        curator.require_auth();
        let key = Self::curator_key(&env, &curator);
        if env.storage().persistent().has(&key) {
            panic!("curator already registered");
        }
        let state = CuratorState {
            stake: MIN_CURATOR_STAKE,
            status: CuratorStatus::Active,
        };
        env.storage().persistent().set(&key, &state);
        env.storage()
            .persistent()
            .extend_ttl(&key, 7_776_000, 7_776_000);

        let cnt: u32 = env
            .storage()
            .instance()
            .get(&symbol_short!("cur_cnt"))
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&symbol_short!("cur_cnt"), &(cnt + 1));

        env.events()
            .publish((symbol_short!("oracle"), symbol_short!("curator")), curator);
    }

    /// Submit a quality score attestation for a dataset.
    pub fn attest_quality(
        env: Env,
        curator: Address,
        dataset_id: String,
        score: u32,
        rubric_hash: soroban_sdk::BytesN<32>,
    ) {
        Self::require_not_paused(&env);
        curator.require_auth();

        // Validate curator is registered and in good standing
        let cur_key = Self::curator_key(&env, &curator);
        let cur_state: CuratorState = env
            .storage()
            .persistent()
            .get(&cur_key)
            .expect("curator not registered");
        if cur_state.status == CuratorStatus::Banned {
            panic!("curator is banned");
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
        let attest_key = Self::attestation_key(&env, &dataset_id, &curator);
        env.storage().persistent().set(&attest_key, &attest);
        env.storage()
            .persistent()
            .extend_ttl(&attest_key, 7_776_000, 7_776_000);

        // Track which curators have attested to this dataset, so consensus
        // (median) can be recomputed later for slashing.
        let list_key = Self::attester_list_key(&env, &dataset_id);
        let mut attesters: Vec<Address> = env
            .storage()
            .persistent()
            .get(&list_key)
            .unwrap_or(Vec::new(&env));
        if !attesters.contains(&curator) {
            attesters.push_back(curator.clone());
        }
        env.storage().persistent().set(&list_key, &attesters);
        env.storage()
            .persistent()
            .extend_ttl(&list_key, 7_776_000, 7_776_000);

        // Update aggregate score
        let agg_key = String::from_str(&env, &format!("agg_{:?}", dataset_id));
        let mut quality: DatasetQuality =
            env.storage()
                .persistent()
                .get(&agg_key)
                .unwrap_or(DatasetQuality {
                    dataset_id: dataset_id.clone(),
                    average_score: 0,
                    attestation_count: 0,
                    last_updated_ledger: 0,
                    tier: QualityTier::Unrated,
                });

        // Running average
        let new_total =
            quality.average_score as u64 * quality.attestation_count as u64 + score as u64;
        quality.attestation_count += 1;
        quality.average_score = (new_total / quality.attestation_count as u64) as u32;
        quality.last_updated_ledger = env.ledger().sequence();
        quality.tier = Self::compute_tier(quality.average_score);

        env.storage().persistent().set(&agg_key, &quality);
        env.storage()
            .persistent()
            .extend_ttl(&agg_key, 7_776_000, 7_776_000);

        env.events().publish(
            (symbol_short!("oracle"), symbol_short!("attested")),
            (dataset_id, curator, score, quality.tier),
        );
    }

    /// Get aggregate quality for a dataset.
    pub fn get_quality(env: Env, dataset_id: String) -> DatasetQuality {
        let agg_key = String::from_str(&env, &format!("agg_{:?}", dataset_id));
        env.storage()
            .persistent()
            .get(&agg_key)
            .expect("no quality data for dataset")
    }

    /// Get a curator's stake and standing.
    pub fn get_curator(env: Env, curator: Address) -> CuratorState {
        let key = Self::curator_key(&env, &curator);
        env.storage()
            .persistent()
            .get(&key)
            .expect("curator not registered")
    }

    /// Running total of stroops swept to the protocol treasury via slashing.
    pub fn treasury_balance(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&symbol_short!("treasury"))
            .unwrap_or(0)
    }

    /// Slash a curator whose attestation for `dataset_id` deviates from
    /// consensus (the median of every attestation on that dataset) by more
    /// than `SLASH_DEVIATION_THRESHOLD` points. Admin only.
    ///
    /// First offense moves the curator to `SlashWarning`; a second offense
    /// moves them to `Banned`, after which they can no longer attest. Each
    /// slash burns 20% of the curator's remaining stake to the protocol
    /// treasury balance.
    pub fn slash_curator(env: Env, curator: Address, dataset_id: String) {
        Self::require_not_paused(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();

        let cur_key = Self::curator_key(&env, &curator);
        let mut cur_state: CuratorState = env
            .storage()
            .persistent()
            .get(&cur_key)
            .expect("curator not registered");
        if cur_state.status == CuratorStatus::Banned {
            panic!("curator already banned");
        }

        let attest_key = Self::attestation_key(&env, &dataset_id, &curator);
        let attestation: QualityAttestation = env
            .storage()
            .persistent()
            .get(&attest_key)
            .expect("curator has no attestation for this dataset");

        let list_key = Self::attester_list_key(&env, &dataset_id);
        let attesters: Vec<Address> = env
            .storage()
            .persistent()
            .get(&list_key)
            .expect("no attestations for dataset");

        let mut scores: alloc::vec::Vec<u32> = alloc::vec::Vec::new();
        for attester in attesters.iter() {
            let key = Self::attestation_key(&env, &dataset_id, &attester);
            let a: QualityAttestation = env
                .storage()
                .persistent()
                .get(&key)
                .expect("missing attestation for listed attester");
            scores.push(a.score);
        }
        scores.sort_unstable();
        let mid = scores.len() / 2;
        let consensus = if scores.len().is_multiple_of(2) {
            (scores[mid - 1] + scores[mid]) / 2
        } else {
            scores[mid]
        };

        let deviation = attestation.score.abs_diff(consensus);
        if deviation <= SLASH_DEVIATION_THRESHOLD {
            panic!("attestation within consensus tolerance, cannot slash");
        }

        let slash_amount = cur_state.stake * SLASH_BPS / TOTAL_BPS;
        cur_state.stake -= slash_amount;
        cur_state.status = match cur_state.status {
            CuratorStatus::Active => CuratorStatus::SlashWarning,
            CuratorStatus::SlashWarning => CuratorStatus::Banned,
            CuratorStatus::Banned => unreachable!(),
        };
        env.storage().persistent().set(&cur_key, &cur_state);
        env.storage()
            .persistent()
            .extend_ttl(&cur_key, 7_776_000, 7_776_000);

        let treasury_key = symbol_short!("treasury");
        let treasury_balance: i128 = env.storage().instance().get(&treasury_key).unwrap_or(0);
        env.storage()
            .instance()
            .set(&treasury_key, &(treasury_balance + slash_amount));

        env.events().publish(
            (symbol_short!("oracle"), symbol_short!("slashed")),
            (curator, dataset_id, slash_amount, cur_state.status),
        );
    }

    /// Extend a dataset's quality-aggregate TTL. Permissionless — anyone
    /// may call this to keep a dataset's quality record (and the royalty
    /// tier it feeds into) from expiring off persistent storage.
    pub fn renew_quality_ttl(env: Env, dataset_id: String) {
        let agg_key = String::from_str(&env, &format!("agg_{:?}", dataset_id));
        if !env.storage().persistent().has(&agg_key) {
            panic!("no quality data for dataset");
        }
        env.storage()
            .persistent()
            .extend_ttl(&agg_key, 7_776_000, 7_776_000);
    }

    /// Compute royalty multiplier (bps) based on quality tier.
    /// Platinum = 150% (1.5x), Gold = 125%, Silver = 100%, Bronze = 75%
    pub fn royalty_multiplier_bps(env: Env, dataset_id: String) -> u32 {
        let agg_key = String::from_str(&env, &format!("agg_{:?}", dataset_id));
        match env
            .storage()
            .persistent()
            .get::<String, DatasetQuality>(&agg_key)
        {
            Some(q) => match q.tier {
                QualityTier::Platinum => 15000,
                QualityTier::Gold => 12500,
                QualityTier::Silver => 10000,
                QualityTier::Bronze => 7500,
                QualityTier::Unrated => 10000,
            },
            None => 10000,
        }
    }

    fn curator_key(env: &Env, curator: &Address) -> String {
        String::from_str(env, &format!("cur_{:?}", curator))
    }

    fn attestation_key(env: &Env, dataset_id: &String, curator: &Address) -> String {
        String::from_str(env, &format!("att_{:?}_{:?}", dataset_id, curator))
    }

    fn attester_list_key(env: &Env, dataset_id: &String) -> String {
        String::from_str(env, &format!("alist_{:?}", dataset_id))
    }

    fn compute_tier(score: u32) -> QualityTier {
        match score {
            0 => QualityTier::Unrated,
            1..=39 => QualityTier::Bronze,
            40..=69 => QualityTier::Silver,
            70..=84 => QualityTier::Gold,
            _ => QualityTier::Platinum,
        }
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
        env.deployer()
            .update_current_contract_wasm(new_wasm_hash.clone());
        env.events().publish(
            (symbol_short!("contract"), symbol_short!("upgraded")),
            (new_wasm_hash, env.ledger().sequence()),
        );
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod pause_test;
