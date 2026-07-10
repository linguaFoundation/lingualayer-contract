#![no_std]
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, token,
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

        let record = PayoutRecord {
            dataset_id: dataset_id.clone(),
            total_amount,
            ledger: env.ledger().sequence(),
            tx_count: count + 1,
        };

        let key = String::from_str(&env, &format!("pay_{}", count + 1));
        env.storage().persistent().set(&key, &record);
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

    pub fn version(_env: Env) -> u32 {
        2
    }
}
