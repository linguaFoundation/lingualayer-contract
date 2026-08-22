#![no_std]
extern crate alloc;
use alloc::format;
use soroban_sdk::{
    contract, contractclient, contractimpl, contracttype, symbol_short, token,
    Address, Env, String, Vec,
};

#[contracttype]
#[derive(Clone, Debug)]
pub struct SplitConfig {
    pub dataset_id: String,
    pub token: Address,        // SAC USDC address
    pub treasury: Address,     // Protocol treasury (5% fee)
    pub contributors: Vec<(Address, u32)>, // (address, share_bps)
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct PayoutRecord {
    pub dataset_id: String,
    pub total_amount: i128,
    pub ledger: u32,
    pub tx_count: u32,
    pub quality_tier: String, // "Platinum"/"Gold"/"Silver"/"Bronze"/"Unrated", read from QualityOracle at payout time
}

/// Mirrors quality-oracle's QualityTier enum shape exactly (wire-compatible
/// via XDR) — kept local rather than depending on the quality-oracle crate
/// directly, which would pull its own #[contractimpl]-generated WASM
/// exports into this contract's binary and collide at link time with
/// royalty-splitter's own exports of the same names.
#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub enum QualityTier {
    Unrated,
    Bronze,
    Silver,
    Gold,
    Platinum,
}

/// Mirrors quality-oracle's DatasetQuality shape — only used to receive the
/// cross-contract get_quality() response; only the `tier` field is read.
#[contracttype]
#[derive(Clone, Debug)]
pub struct DatasetQuality {
    pub dataset_id: String,
    pub average_score: u32,
    pub attestation_count: u32,
    pub last_updated_ledger: u32,
    pub tier: QualityTier,
}

#[contractclient(name = "QualityOracleClient")]
pub trait QualityOracleInterface {
    fn get_quality(env: Env, dataset_id: String) -> DatasetQuality;
}

fn tier_label(env: &Env, tier: &QualityTier) -> String {
    match tier {
        QualityTier::Platinum => String::from_str(env, "Platinum"),
        QualityTier::Gold => String::from_str(env, "Gold"),
        QualityTier::Silver => String::from_str(env, "Silver"),
        QualityTier::Bronze => String::from_str(env, "Bronze"),
        QualityTier::Unrated => String::from_str(env, "Unrated"),
    }
}

/// Revenue distribution — splits license fees to contributors on-chain.
#[contract]
pub struct RoyaltySplitter;

#[contractimpl]
impl RoyaltySplitter {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&symbol_short!("admin")) {
            panic!("already initialized");
        }
        admin.require_auth();
        env.storage().instance().set(&symbol_short!("admin"), &admin);
        env.storage().instance().set(&symbol_short!("pay_cnt"), &0u32);
    }

    /// Register a royalty split configuration for a dataset.
    pub fn register_split(env: Env, config: SplitConfig) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();

        // Validate shares
        let total: u32 = config.contributors.iter().map(|(_, bps)| bps).sum();
        if total != 10000 {
            panic!("contributor shares must sum to 10000 bps");
        }

        env.storage()
            .persistent()
            .set(&config.dataset_id, &config);
    }

    /// Execute a royalty payout for a dataset from accumulated fees.
    /// Deducts 5% protocol treasury fee then splits remainder.
    pub fn distribute(env: Env, dataset_id: String, total_amount: i128) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();

        let config: SplitConfig = env
            .storage()
            .persistent()
            .get(&dataset_id)
            .expect("split config not found");

        if total_amount <= 0 {
            panic!("amount must be positive");
        }

        let token_client = token::Client::new(&env, &config.token);

        // 5% treasury fee
        let treasury_fee = total_amount * 500 / 10000;
        let distributable = total_amount - treasury_fee;

        token_client.transfer(
            &env.current_contract_address(),
            &config.treasury,
            &treasury_fee,
        );

        // Split remainder to contributors
        for (contributor, share_bps) in config.contributors.iter() {
            let payout = distributable * (share_bps as i128) / 10000;
            if payout > 0 {
                token_client.transfer(
                    &env.current_contract_address(),
                    &contributor,
                    &payout,
                );
            }
        }

        let count: u32 = env
            .storage()
            .instance()
            .get(&symbol_short!("pay_cnt"))
            .unwrap_or(0);

        // Read the dataset's quality tier at payout time for the receipt.
        // No oracle configured, or the dataset never attested (get_quality
        // panics in that case), defaults to "Unrated" rather than blocking
        // the payout.
        let quality_tier = match env
            .storage()
            .instance()
            .get::<_, Address>(&symbol_short!("oracle"))
        {
            Some(oracle_contract) => {
                let oracle = QualityOracleClient::new(&env, &oracle_contract);
                match oracle.try_get_quality(&dataset_id) {
                    Ok(Ok(quality)) => tier_label(&env, &quality.tier),
                    _ => String::from_str(&env, "Unrated"),
                }
            }
            None => String::from_str(&env, "Unrated"),
        };

        let record = PayoutRecord {
            dataset_id: dataset_id.clone(),
            total_amount,
            ledger: env.ledger().sequence(),
            tx_count: count + 1,
            quality_tier: quality_tier.clone(),
        };

        let key = String::from_str(&env, &format!("pay_{}", count + 1));
        env.storage().persistent().set(&key, &record);
        env.storage()
            .instance()
            .set(&symbol_short!("pay_cnt"), &(count + 1));

        env.events().publish(
            (symbol_short!("royalty"), symbol_short!("paid")),
            (dataset_id, total_amount, env.ledger().sequence(), quality_tier),
        );
    }

    /// Total historical payouts recorded.
    pub fn payout_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("pay_cnt"))
            .unwrap_or(0)
    }

    /// Read a payout receipt by its 1-based sequence number (as returned by
    /// `payout_count` after the corresponding `distribute` call).
    pub fn get_payout(env: Env, tx_count: u32) -> PayoutRecord {
        let key = String::from_str(&env, &format!("pay_{}", tx_count));
        env.storage()
            .persistent()
            .get(&key)
            .expect("payout not found")
    }

    /// Configure (or update) the QualityOracle contract used to read each
    /// dataset's quality tier at payout time. Admin only.
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

    pub fn version(_env: Env) -> u32 {
        2
    }
}

#[cfg(test)]
mod test;
