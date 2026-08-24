#![no_std]
use soroban_sdk::{contract, contractimpl, Env};

#[contract]
pub struct Dummy;

#[contractimpl]
impl Dummy {
    pub fn hello(_env: Env) {}
}
