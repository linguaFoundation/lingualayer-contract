#![no_std]
extern crate alloc;
use alloc::format;
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, token, Address, Env, String, Vec,
};

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
            .set(&symbol_short!("pay_cnt"), &0u32);
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

        let dataset_id = config.dataset_id.clone();
        env.storage().persistent().set(&dataset_id, &config);
        env.storage()
            .persistent()
            .extend_ttl(&dataset_id, PERSISTENT_TTL, PERSISTENT_TTL);
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
            panic!("insufficient contract balance for distribution");
        }

        token_client.transfer(&contract, &config.treasury, &treasury_fee);

        for (i, (contributor, _)) in config.contributors.iter().enumerate() {
            let payout = payouts.get(i as u32).unwrap_or(0);
            if payout > 0 {
                token_client.transfer(&contract, &contributor, &payout);
            }
        }

        let count: u32 = env
            .storage()
            .instance()
            .get(&symbol_short!("pay_cnt"))
            .unwrap_or(0);

        let record = PayoutRecord {
            dataset_id: dataset_id.clone(),
            total_amount,
            ledger: env.ledger().sequence(),
            tx_count: count + 1,
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
            (dataset_id, total_amount, env.ledger().sequence()),
        );
    }

    /// Total historical payouts recorded.
    pub fn payout_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("pay_cnt"))
            .unwrap_or(0)
    }

    /// Read back a historical payout by its 1-based sequence number.
    pub fn get_payout(env: Env, index: u32) -> PayoutRecord {
        let key = String::from_str(&env, &format!("pay_{}", index));
        env.storage()
            .persistent()
            .get(&key)
            .expect("payout not found")
    }

    /// The registered split configuration for a dataset.
    pub fn get_split(env: Env, dataset_id: String) -> SplitConfig {
        env.storage()
            .persistent()
            .get(&dataset_id)
            .expect("split config not found")
    }

    pub fn version(_env: Env) -> u32 {
        2
    }
}

#[cfg(test)]
mod test;
