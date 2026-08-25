#![no_std]
extern crate alloc;
use alloc::format;
use soroban_sdk::{
    contracterror,
    contract, contractclient, contractimpl, contracttype, symbol_short, token, Address, Env,
    String, Vec,
};


#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NoAdminProposalPending = 3,
    SharesMustSumTo10000 = 4,
    SplitConfigNotFound = 5,
    AmountMustBePositive = 6,
    InsufficientContractBalance = 7,
    PayoutNotFound = 8,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct SplitConfig {
    pub dataset_id: String,
    pub token: Address,                    // SAC USDC address
    pub treasury: Address,                 // Protocol treasury (5% fee)
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
    /// Must stay in step with quality-oracle's struct: this is a wire shape,
    /// and a field missing here silently fails to decode the response.
    pub needs_more_attestations: bool,
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

/// How long persistent entries live before they can be archived:
/// 7,776,000 ledgers ≈ 90 days at ~1s/ledger.
const PERSISTENT_TTL: u32 = 7_776_000;

/// Total contributor shares must add up to this, in basis points.
const TOTAL_BPS: i128 = 10_000;

/// Protocol treasury fee, in basis points (5%).
const TREASURY_BPS: i128 = 500;

/// Revenue distribution — splits license fees to contributors on-chain.
#[contract]
pub struct RoyaltySplitter;

#[contractimpl]
impl RoyaltySplitter {
    pub fn initialize(env: Env, admin: Address) -> Result<(), Error> {
        if env.storage().instance().has(&symbol_short!("admin")) {
            return Err(Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("admin"), &admin);
        env.storage()
            .instance()
            .set(&symbol_short!("pay_cnt"), &0u32);
        Self::bump_instance(&env);
        Ok(())
    }

    /// Step 1 of admin handoff: current admin proposes a successor. The
    /// proposal must be accepted by the new admin via `accept_admin` before
    /// control actually transfers — a compromised key alone can't hand
    /// itself off without the new admin's own signature.
    pub fn propose_admin(env: Env, new_admin: Address) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .ok_or(Error::NotInitialized)?;
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

    /// Register a royalty split configuration for a dataset.
    pub fn register_split(env: Env, config: SplitConfig) -> Result<(), Error> {
        Self::bump_instance(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        // Validate shares
        let total: u32 = config.contributors.iter().map(|(_, bps)| bps).sum();
        if total != 10000 {
            return Err(Error::SharesMustSumTo10000);
        }

        let dataset_id = config.dataset_id.clone();
        env.storage().persistent().set(&dataset_id, &config);
        env.storage()
            .persistent()
            .extend_ttl(&dataset_id, PERSISTENT_TTL, PERSISTENT_TTL);
        Ok(())
    }

    /// Execute a royalty payout for a dataset from accumulated fees.
    /// Deducts 5% protocol treasury fee then splits remainder.
    pub fn distribute(env: Env, dataset_id: String, total_amount: i128) -> Result<(), Error> {
        Self::bump_instance(&env);
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let config: SplitConfig = env
            .storage()
            .persistent()
            .get(&dataset_id)
            .ok_or(Error::SplitConfigNotFound)?;

        if total_amount <= 0 {
            return Err(Error::AmountMustBePositive);
        }

        let token_client = token::Client::new(&env, &config.token);

        // 5% treasury fee, floored — the truncated fraction stays with the
        // contributors rather than the protocol.
        let treasury_fee = total_amount * TREASURY_BPS / TOTAL_BPS;
        let distributable = total_amount - treasury_fee;

        // Integer division floors every contributor payout, so the naive
        // per-share loop leaves up to `contributors.len() - 1` stroops
        // stranded in the contract on each distribution. Dust that never
        // leaves accumulates across payouts and silently breaks the
        // invariant that contributors receive exactly `total - fee`, so the
        // shortfall is reconciled onto the largest shareholder — the
        // deterministic choice, and the party whose floored payout absorbed
        // the most truncation.
        let mut payouts: Vec<i128> = Vec::new(&env);
        let mut allocated: i128 = 0;
        let mut largest_index: u32 = 0;
        let mut largest_bps: u32 = 0;

        for (i, (_, share_bps)) in config.contributors.iter().enumerate() {
            let payout = distributable * (share_bps as i128) / TOTAL_BPS;
            payouts.push_back(payout);
            allocated += payout;
            if share_bps > largest_bps {
                largest_bps = share_bps;
                largest_index = i as u32;
            }
        }

        let dust = distributable - allocated;
        if dust > 0 {
            let adjusted = payouts.get(largest_index).unwrap_or(0) + dust;
            payouts.set(largest_index, adjusted);
        }

        // Refuse to start transferring unless the contract can cover the
        // whole distribution. Without this a short balance fails partway
        // through, leaving some contributors paid and the rest not, with the
        // payout still recorded as if it had completed.
        let contract = env.current_contract_address();
        if token_client.balance(&contract) < total_amount {
            return Err(Error::InsufficientContractBalance);
        }

        token_client.transfer(&contract, &config.treasury, &treasury_fee);

        for (i, (contributor, _)) in config.contributors.iter().enumerate() {
            let payout = payouts.get(i as u32).unwrap_or(0);
            if payout > 0 {
                token_client.transfer(&env.current_contract_address(), &contributor, &payout);
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
            .persistent()
            .extend_ttl(&key, PERSISTENT_TTL, PERSISTENT_TTL);
        env.storage()
            .instance()
            .set(&symbol_short!("pay_cnt"), &(count + 1));

        env.events().publish(
            (symbol_short!("royalty"), symbol_short!("paid")),
            (
                dataset_id,
                total_amount,
                env.ledger().sequence(),
                quality_tier,
            ),
        );
        Ok(())
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
    pub fn get_payout(env: Env, tx_count: u32) -> Result<PayoutRecord, Error> {
        let key = String::from_str(&env, &format!("pay_{}", tx_count));
        env.storage()
            .persistent()
            .get(&key)
            .ok_or(Error::PayoutNotFound)?
    }

    /// Configure (or update) the QualityOracle contract used to read each
    /// dataset's quality tier at payout time. Admin only.
    pub fn set_oracle(env: Env, oracle_contract: Address) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("oracle"), &oracle_contract);
        Ok(())
    }

    /// Refresh the contract's own instance entry on every storage-touching
    /// call. Instance storage holds the admin, the oracle address and the
    /// payout counter, and if it lapses the whole
    /// contract is archived — every record underneath it becomes unreadable
    /// even with a perfectly fresh TTL of its own.
    fn bump_instance(env: &Env) {
        env.storage()
            .instance()
            .extend_ttl(PERSISTENT_TTL, PERSISTENT_TTL);
    }

    /// Permissionlessly extend a split configuration's storage lifetime.
    ///
    /// A lapsed `SplitConfig` is worse than a lapsed record elsewhere: it does
    /// not fail closed. `distribute` reads the config to decide who gets paid,
    /// so if it expires the payout path for that dataset stops working
    /// entirely until someone re-registers the shares by hand — and whoever
    /// re-registers them decides what they are. Keeping renewal open to anyone
    /// means any contributor in the split can protect their own claim.
    pub fn renew_split_ttl(env: Env, dataset_id: String) -> Result<(), Error> {
        Self::bump_instance(&env);
        if !env.storage().persistent().has(&dataset_id) {
            return Err(Error::SplitConfigNotFound);
        }
        env.storage()
            .persistent()
            .extend_ttl(&dataset_id, PERSISTENT_TTL, PERSISTENT_TTL);
        Ok(())
    }

    /// Permissionlessly extend a single payout receipt's storage lifetime.
    /// Receipts are the only on-chain evidence that a distribution happened
    /// and at what quality tier, so they outlive any one caller's interest.
    pub fn renew_payout_ttl(env: Env, tx_count: u32) -> Result<(), Error> {
        Self::bump_instance(&env);
        let key = String::from_str(&env, &format!("pay_{}", tx_count));
        if !env.storage().persistent().has(&key) {
            return Err(Error::PayoutNotFound);
        }
        env.storage()
            .persistent()
            .extend_ttl(&key, PERSISTENT_TTL, PERSISTENT_TTL);
        Ok(())
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
