#![no_std]
#![allow(clippy::too_many_arguments)]
extern crate alloc;
use alloc::format;
use soroban_sdk::{
    contract, contractimpl, contracttype, symbol_short, token, Address, Env, String, Vec,
};

/// A single tranche of a commission's bounty, released independently once
/// its deliverable is verified — instead of the fulfiller waiting for the
/// entire dataset before any payout, or the commissioner releasing the full
/// bounty on a single unverified handoff.
#[contracttype]
#[derive(Clone, Debug)]
pub struct Milestone {
    pub description_hash: soroban_sdk::BytesN<32>, // IPFS doc for this tranche's deliverable
    pub amount: i128,
    pub released: bool,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CommissionState {
    Open,
    Fulfilled,
    Cancelled,
    Expired,
    Disputed,
}

#[contracttype]
#[derive(Clone, Debug)]
pub struct Commission {
    pub id: String,
    pub commissioner: Address, // AI company posting the bounty
    pub language_code: String, // ISO 639-3 target language
    pub description_hash: soroban_sdk::BytesN<32>, // IPFS requirements doc
    pub bounty_token: Address, // USDC SAC address
    pub bounty_amount: i128,   // Total bounty in stroops
    // Empty until set_milestones is called; while empty, fulfil_commission
    // releases the full bounty_amount in one shot (unchanged legacy path).
    pub milestones: Vec<Milestone>,
    pub min_sample_count: u32,     // Minimum audio samples required
    pub min_duration_seconds: u32, // Minimum total duration
    pub deadline_ledger: u32,
    pub state: CommissionState,
    pub fulfiller: Option<Address>, // Dataset contributor who won
    pub fulfilled_dataset_id: Option<String>,
}

/// Dataset commissioning escrow — AI companies post bounties for specific
/// language data; contributors fulfil and claim the bounty on delivery.
#[contract]
pub struct DataCommission;

#[contractimpl]
impl DataCommission {
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
            .set(&symbol_short!("com_cnt"), &0u32);
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

    /// Post a new data commission with USDC bounty.
    pub fn post_commission(
        env: Env,
        commissioner: Address,
        language_code: String,
        description_hash: soroban_sdk::BytesN<32>,
        bounty_token: Address,
        bounty_amount: i128,
        min_sample_count: u32,
        min_duration_seconds: u32,
        deadline_ledger: u32,
    ) -> String {
        commissioner.require_auth();

        if description_hash == soroban_sdk::BytesN::from_array(&env, &[0u8; 32]) {
            panic!("metadata hash cannot be zero");
        }

        if bounty_amount <= 0 {
            panic!("bounty must be positive");
        }
        if deadline_ledger <= env.ledger().sequence() {
            panic!("deadline must be in the future");
        }

        // Transfer bounty into contract escrow
        let tok = token::Client::new(&env, &bounty_token);
        tok.transfer(
            &commissioner,
            &env.current_contract_address(),
            &bounty_amount,
        );

        let cnt: u32 = env
            .storage()
            .instance()
            .get(&symbol_short!("com_cnt"))
            .unwrap_or(0);
        let id = String::from_str(&env, &format!("com_{}", cnt + 1));

        let commission = Commission {
            id: id.clone(),
            commissioner,
            language_code: language_code.clone(),
            description_hash,
            bounty_token,
            bounty_amount,
            milestones: Vec::new(&env),
            min_sample_count,
            min_duration_seconds,
            deadline_ledger,
            state: CommissionState::Open,
            fulfiller: None,
            fulfilled_dataset_id: None,
        };

        env.storage().persistent().set(&id, &commission);
        env.storage()
            .persistent()
            .extend_ttl(&id, 7_776_000, 7_776_000);
        env.storage()
            .instance()
            .set(&symbol_short!("com_cnt"), &(cnt + 1));

        env.events().publish(
            (symbol_short!("comm"), symbol_short!("posted")),
            (id.clone(), language_code, bounty_amount),
        );

        id
    }

    /// Fulfil a commission — admin verifies delivery and releases escrow.
    pub fn fulfil_commission(
        env: Env,
        commission_id: String,
        fulfiller: Address,
        dataset_id: String,
    ) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();

        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .expect("commission not found");

        if comm.state != CommissionState::Open {
            panic!("commission not open");
        }
        if env.ledger().sequence() > comm.deadline_ledger {
            panic!("commission deadline passed");
        }

        comm.fulfiller = Some(fulfiller.clone());
        comm.fulfilled_dataset_id = Some(dataset_id.clone());

        if comm.milestones.is_empty() {
            // Legacy path: no milestones were set, release the full bounty
            // immediately, exactly as before milestone support existed.
            let tok = token::Client::new(&env, &comm.bounty_token);
            tok.transfer(
                &env.current_contract_address(),
                &fulfiller,
                &comm.bounty_amount,
            );
            comm.state = CommissionState::Fulfilled;
        }
        // Milestone path: state stays Open and funds stay in escrow until
        // release_milestone pays out each tranche; the last release flips
        // state to Fulfilled (see release_milestone).

        env.storage().persistent().set(&commission_id, &comm);

        env.events().publish(
            (symbol_short!("comm"), symbol_short!("fulfilled")),
            (commission_id, fulfiller, dataset_id, comm.bounty_amount),
        );
    }

    /// Cancel an expired commission and refund the commissioner.
    pub fn cancel_commission(env: Env, commission_id: String) {
        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .expect("commission not found");

        if comm.state != CommissionState::Open {
            panic!("commission not open");
        }

        // Only cancel if past deadline
        if env.ledger().sequence() <= comm.deadline_ledger {
            comm.commissioner.require_auth(); // owner can cancel early
        }

        // Refund whatever is still held in escrow — for a milestone
        // commission that's partially released, that's the bounty minus
        // whatever tranches already paid out, not the original total.
        let released: i128 = comm
            .milestones
            .iter()
            .filter(|m| m.released)
            .map(|m| m.amount)
            .sum();
        let remaining = comm.bounty_amount - released;
        if remaining > 0 {
            let tok = token::Client::new(&env, &comm.bounty_token);
            tok.transfer(
                &env.current_contract_address(),
                &comm.commissioner,
                &remaining,
            );
        }

        comm.state = CommissionState::Cancelled;
        env.storage().persistent().set(&commission_id, &comm);

        env.events().publish(
            (symbol_short!("comm"), symbol_short!("cancelled")),
            commission_id,
        );
    }

    /// Split a commission's bounty into independently-released tranches.
    /// Must be called before fulfil_commission — once a fulfiller is
    /// assigned, the milestone set for that commission is locked in.
    pub fn set_milestones(env: Env, commission_id: String, milestones: Vec<Milestone>) {
        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .expect("commission not found");

        comm.commissioner.require_auth();

        if comm.state != CommissionState::Open || comm.fulfiller.is_some() {
            panic!("commission already fulfilled");
        }
        if milestones.is_empty() {
            panic!("must provide at least one milestone");
        }

        let mut total: i128 = 0;
        for m in milestones.iter() {
            if m.amount <= 0 {
                panic!("milestone amount must be positive");
            }
            if m.released {
                panic!("new milestones cannot start released");
            }
            total += m.amount;
        }
        if total != comm.bounty_amount {
            panic!("milestone amounts must sum to the bounty amount");
        }

        comm.milestones = milestones;
        env.storage().persistent().set(&commission_id, &comm);

        env.events().publish(
            (symbol_short!("comm"), symbol_short!("mstones")),
            commission_id,
        );
    }

    /// Release one milestone's tranche to the fulfiller. Admin-gated for the
    /// same reason fulfil_commission is: the admin is the party attesting
    /// that the off-chain deliverable for this tranche was actually
    /// verified. The final milestone's release also marks the commission
    /// Fulfilled.
    pub fn release_milestone(env: Env, commission_id: String, milestone_index: u32) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();

        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .expect("commission not found");

        if comm.state != CommissionState::Open {
            panic!("commission not open");
        }
        let fulfiller = comm
            .fulfiller
            .clone()
            .expect("commission has no fulfiller yet");

        let idx = milestone_index as usize;
        if idx >= comm.milestones.len() as usize {
            panic!("milestone index out of range");
        }
        let mut milestone = comm
            .milestones
            .get(milestone_index)
            .expect("milestone index out of range");
        if milestone.released {
            panic!("milestone already released");
        }

        let tok = token::Client::new(&env, &comm.bounty_token);
        tok.transfer(
            &env.current_contract_address(),
            &fulfiller,
            &milestone.amount,
        );

        milestone.released = true;
        comm.milestones.set(milestone_index, milestone.clone());

        let all_released = comm.milestones.iter().all(|m| m.released);
        if all_released {
            comm.state = CommissionState::Fulfilled;
        }
        env.storage().persistent().set(&commission_id, &comm);

        env.events().publish(
            (symbol_short!("comm"), symbol_short!("mrelease")),
            (commission_id, milestone_index, milestone.amount),
        );
    }

    pub fn get_commission(env: Env, commission_id: String) -> Commission {
        env.storage()
            .persistent()
            .get(&commission_id)
            .expect("commission not found")
    }

    /// Extend a commission's storage TTL. Permissionless — anyone may call
    /// this to keep a commission they care about from expiring off
    /// persistent storage; extending an entry's lifetime can't be abused
    /// the way mutating it could, so there's no auth requirement.
    pub fn renew_commission_ttl(env: Env, commission_id: String) {
        if !env.storage().persistent().has(&commission_id) {
            panic!("commission not found");
        }
        env.storage()
            .persistent()
            .extend_ttl(&commission_id, 7_776_000, 7_776_000);
    }

    pub fn commission_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("com_cnt"))
            .unwrap_or(0)
    }

    pub fn version(_env: Env) -> u32 {
        1
    }

    /// Designate the address that can rule on disputed commissions.
    /// Admin-gated; callable again to rotate the arbiter.
    pub fn set_arbiter(env: Env, arbiter: Address) {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .expect("not initialized");
        admin.require_auth();

        env.storage()
            .instance()
            .set(&symbol_short!("arbiter"), &arbiter);

        env.events()
            .publish((symbol_short!("comm"), symbol_short!("arbiter")), arbiter);
    }

    /// The commissioner flags a commission as disputed — e.g. they believe
    /// the fulfiller's delivery doesn't meet the commission's requirements.
    /// Only valid while Open (before the admin has released any funds);
    /// freezes the commission until the arbiter rules on it.
    pub fn raise_dispute(env: Env, commission_id: String, raised_by: Address) {
        raised_by.require_auth();

        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .expect("commission not found");

        if raised_by != comm.commissioner {
            panic!("only the commissioner can raise a dispute");
        }
        if comm.state != CommissionState::Open {
            panic!("commission not open");
        }

        comm.state = CommissionState::Disputed;
        env.storage().persistent().set(&commission_id, &comm);

        env.events().publish(
            (symbol_short!("comm"), symbol_short!("disputed")),
            commission_id,
        );
    }

    /// The arbiter rules on a disputed commission: either award the full
    /// bounty to the fulfiller (delivery was acceptable) or refund the
    /// commissioner (it wasn't). This is a binary, whole-bounty ruling —
    /// it doesn't compose with milestone-partial payouts.
    pub fn resolve_dispute(
        env: Env,
        commission_id: String,
        award_to_fulfiller: bool,
        fulfiller: Address,
        dataset_id: String,
    ) {
        let arbiter: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("arbiter"))
            .expect("no arbiter set");
        arbiter.require_auth();

        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .expect("commission not found");

        if comm.state != CommissionState::Disputed {
            panic!("commission not disputed");
        }

        let tok = token::Client::new(&env, &comm.bounty_token);

        if award_to_fulfiller {
            tok.transfer(
                &env.current_contract_address(),
                &fulfiller,
                &comm.bounty_amount,
            );
            comm.state = CommissionState::Fulfilled;
            comm.fulfiller = Some(fulfiller.clone());
            comm.fulfilled_dataset_id = Some(dataset_id.clone());
        } else {
            tok.transfer(
                &env.current_contract_address(),
                &comm.commissioner,
                &comm.bounty_amount,
            );
            comm.state = CommissionState::Cancelled;
        }
        env.storage().persistent().set(&commission_id, &comm);

        env.events().publish(
            (symbol_short!("comm"), symbol_short!("resolved")),
            (commission_id, award_to_fulfiller),
        );
    }
}

#[cfg(test)]
mod test;
