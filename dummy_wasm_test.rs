use soroban_sdk::{Env, BytesN, contract, contractimpl, symbol_short};

#[contract]
pub struct DummyContract;
#[contractimpl]
impl DummyContract {
    pub fn hello() -> u32 { 1 }
}
