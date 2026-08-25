#![no_std]
#![allow(clippy::too_many_arguments)]
extern crate alloc;
use alloc::format;
use soroban_sdk::{
    contract, contracterror, contractimpl, contracttype, symbol_short, token, Address, Env, String,
    Vec,
};

/// A single tranche of a commission's bounty, released independently once
/// its deliverable is verified — instead of the fulfiller waiting for the
/// entire dataset before any payout, or the commissioner releasing the full
/// bounty on a single unverified handoff.

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq, PartialOrd, Ord)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    NotInitialized = 2,
    NoAdminProposalPending = 3,
    MetadataHashCannotBeZero = 4,
    BountyMustBePositive = 5,
    DeadlineMustBeInTheFuture = 6,
    CommissionNotFound = 7,
    CommissionNotOpen = 8,
    CommissionDeadlinePassed = 9,
    CommissionAlreadyFulfilled = 10,
    MustProvideAtLeastOneMilestone = 11,
    MilestoneAmountMustBePositive = 12,
    NewMilestonesCannotStartReleased = 13,
    MilestoneAmountsMustSumToBounty = 14,
    CommissionHasNoFulfillerYet = 15,
    MilestoneIndexOutOfRange = 16,
    MilestoneAlreadyReleased = 17,
    OnlyCommissionerCanRaiseDispute = 18,
    NoArbiterSet = 19,
    CommissionNotDisputed = 20,
    /// A state-mutating entry point was called while the contract is frozen.
    ContractPaused = 21,
    /// `pause` called on a contract that is already frozen.
    AlreadyPaused = 22,
    /// `unpause` called on a contract that is not frozen.
    NotPaused = 23,
}

/// Storage key used to track per-language commission counts.
///
/// Wrapping the language code in an enum variant keeps it isolated from every
/// other String-keyed entry (commission ids, admin keys, etc.) and makes the
/// intent legible when inspecting raw ledger state.
#[contracttype]
#[derive(Clone, Debug)]
enum LangKey {
    CommissionCount(String),
}

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
            .set(&symbol_short!("com_cnt"), &0u32);
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

    /// Freeze every state-mutating entry point. Admin only.
    ///
    /// This is incident-response machinery: if a vulnerability is found before
    /// or during testnet, the damage window is however long it takes to get a
    /// transaction through, not however long it takes to ship a fix.
    ///
    /// Reads stay available while paused, deliberately. Integrators and the
    /// front end need to keep answering questions about existing state during
    /// an incident, and a read cannot make the problem worse.
    pub fn pause(env: Env) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        if Self::is_paused(env.clone()) {
            return Err(Error::AlreadyPaused);
        }
        env.storage()
            .instance()
            .set(&symbol_short!("paused"), &true);

        env.events().publish(
            (symbol_short!("pause"), symbol_short!("paused")),
            (admin, env.ledger().timestamp()),
        );
        Ok(())
    }

    /// Lift the freeze. Admin only.
    pub fn unpause(env: Env) -> Result<(), Error> {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        if !Self::is_paused(env.clone()) {
            return Err(Error::NotPaused);
        }
        env.storage()
            .instance()
            .set(&symbol_short!("paused"), &false);

        env.events().publish(
            (symbol_short!("pause"), symbol_short!("unpaused")),
            (admin, env.ledger().timestamp()),
        );
        Ok(())
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
    fn require_not_paused(env: &Env) -> Result<(), Error> {
        if Self::is_paused(env.clone()) {
            return Err(Error::ContractPaused);
        }
        Ok(())
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
    ) -> Result<String, Error> {
        Self::require_not_paused(&env)?;
        commissioner.require_auth();

        if description_hash == soroban_sdk::BytesN::from_array(&env, &[0u8; 32]) {
            return Err(Error::MetadataHashCannotBeZero);
        }

        if bounty_amount <= 0 {
            return Err(Error::BountyMustBePositive);
        }
        if deadline_ledger <= env.ledger().sequence() {
            return Err(Error::DeadlineMustBeInTheFuture);
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

        // Increment the per-language commission counter so
        // `commission_count_by_lang` can answer analytics queries without
        // scanning every commission in persistent storage.
        let lang_key = LangKey::CommissionCount(language_code.clone());
        let lang_cnt: u32 = env.storage().instance().get(&lang_key).unwrap_or(0);
        env.storage().instance().set(&lang_key, &(lang_cnt + 1));

        env.events().publish(
            (symbol_short!("comm"), symbol_short!("posted")),
            (id.clone(), language_code, bounty_amount),
        );

        Ok(id)
    }

    /// Fulfil a commission — admin verifies delivery and releases escrow.
    pub fn fulfil_commission(
        env: Env,
        commission_id: String,
        fulfiller: Address,
        dataset_id: String,
    ) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .ok_or(Error::CommissionNotFound)?;

        if comm.state != CommissionState::Open {
            return Err(Error::CommissionNotOpen);
        }
        if env.ledger().sequence() > comm.deadline_ledger {
            return Err(Error::CommissionDeadlinePassed);
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
        Ok(())
    }

    /// Cancel an expired commission and refund the commissioner.
    pub fn cancel_commission(env: Env, commission_id: String) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .ok_or(Error::CommissionNotFound)?;

        if comm.state != CommissionState::Open {
            return Err(Error::CommissionNotOpen);
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
        Ok(())
    }

    /// Split a commission's bounty into independently-released tranches.
    /// Must be called before fulfil_commission — once a fulfiller is
    /// assigned, the milestone set for that commission is locked in.
    pub fn set_milestones(
        env: Env,
        commission_id: String,
        milestones: Vec<Milestone>,
    ) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .ok_or(Error::CommissionNotFound)?;

        comm.commissioner.require_auth();

        if comm.state != CommissionState::Open || comm.fulfiller.is_some() {
            return Err(Error::CommissionAlreadyFulfilled);
        }
        if milestones.is_empty() {
            return Err(Error::MustProvideAtLeastOneMilestone);
        }

        let mut total: i128 = 0;
        for m in milestones.iter() {
            if m.amount <= 0 {
                return Err(Error::MilestoneAmountMustBePositive);
            }
            if m.released {
                return Err(Error::NewMilestonesCannotStartReleased);
            }
            total += m.amount;
        }
        if total != comm.bounty_amount {
            return Err(Error::MilestoneAmountsMustSumToBounty);
        }

        comm.milestones = milestones;
        env.storage().persistent().set(&commission_id, &comm);

        env.events().publish(
            (symbol_short!("comm"), symbol_short!("mstones")),
            commission_id,
        );
        Ok(())
    }

    /// Release one milestone's tranche to the fulfiller. Admin-gated for the
    /// same reason fulfil_commission is: the admin is the party attesting
    /// that the off-chain deliverable for this tranche was actually
    /// verified. The final milestone's release also marks the commission
    /// Fulfilled.
    pub fn release_milestone(
        env: Env,
        commission_id: String,
        milestone_index: u32,
    ) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .ok_or(Error::CommissionNotFound)?;

        if comm.state != CommissionState::Open {
            return Err(Error::CommissionNotOpen);
        }
        let fulfiller = comm
            .fulfiller
            .clone()
            .ok_or(Error::CommissionHasNoFulfillerYet)?;

        let idx = milestone_index as usize;
        if idx >= comm.milestones.len() as usize {
            return Err(Error::MilestoneIndexOutOfRange);
        }
        let mut milestone = comm
            .milestones
            .get(milestone_index)
            .ok_or(Error::MilestoneIndexOutOfRange)?;
        if milestone.released {
            return Err(Error::MilestoneAlreadyReleased);
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
        Ok(())
    }

    pub fn get_commission(env: Env, commission_id: String) -> Result<Commission, Error> {
        env.storage()
            .persistent()
            .get(&commission_id)
            .ok_or(Error::CommissionNotFound)?
    }

    /// Extend a commission's storage TTL. Permissionless — anyone may call
    /// this to keep a commission they care about from expiring off
    /// persistent storage; extending an entry's lifetime can't be abused
    /// the way mutating it could, so there's no auth requirement.
    pub fn renew_commission_ttl(env: Env, commission_id: String) -> Result<(), Error> {
        if !env.storage().persistent().has(&commission_id) {
            return Err(Error::CommissionNotFound);
        }
        env.storage()
            .persistent()
            .extend_ttl(&commission_id, 7_776_000, 7_776_000);
        Ok(())
    }

    pub fn commission_count(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("com_cnt"))
            .unwrap_or(0)
    }

    /// Return the total number of commissions posted for a given ISO 639-3
    /// language code. Returns 0 for a language with no commissions on record,
    /// so callers never need to handle a missing-key error.
    pub fn commission_count_by_lang(env: Env, lang: String) -> u32 {
        let lang_key = LangKey::CommissionCount(lang);
        env.storage().instance().get(&lang_key).unwrap_or(0)
    }

    pub fn version(_env: Env) -> u32 {
        2
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

    /// Designate the address that can rule on disputed commissions.
    /// Admin-gated; callable again to rotate the arbiter.
    pub fn set_arbiter(env: Env, arbiter: Address) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .ok_or(Error::NotInitialized)?;
        admin.require_auth();

        env.storage()
            .instance()
            .set(&symbol_short!("arbiter"), &arbiter);

        env.events()
            .publish((symbol_short!("comm"), symbol_short!("arbiter")), arbiter);
        Ok(())
    }

    /// The commissioner flags a commission as disputed — e.g. they believe
    /// the fulfiller's delivery doesn't meet the commission's requirements.
    /// Only valid while Open (before the admin has released any funds);
    /// freezes the commission until the arbiter rules on it.
    pub fn raise_dispute(env: Env, commission_id: String, raised_by: Address) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        raised_by.require_auth();

        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .ok_or(Error::CommissionNotFound)?;

        if raised_by != comm.commissioner {
            return Err(Error::OnlyCommissionerCanRaiseDispute);
        }
        if comm.state != CommissionState::Open {
            return Err(Error::CommissionNotOpen);
        }

        comm.state = CommissionState::Disputed;
        env.storage().persistent().set(&commission_id, &comm);

        env.events().publish(
            (symbol_short!("comm"), symbol_short!("disputed")),
            commission_id,
        );
        Ok(())
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
    ) -> Result<(), Error> {
        Self::require_not_paused(&env)?;
        let arbiter: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("arbiter"))
            .ok_or(Error::NoArbiterSet)?;
        arbiter.require_auth();

        let mut comm: Commission = env
            .storage()
            .persistent()
            .get(&commission_id)
            .ok_or(Error::CommissionNotFound)?;

        if comm.state != CommissionState::Disputed {
            return Err(Error::CommissionNotDisputed);
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
        Ok(())
    }
}

#[cfg(test)]
mod test;

#[cfg(test)]
mod pause_test;
