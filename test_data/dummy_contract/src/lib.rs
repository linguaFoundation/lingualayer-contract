#![no_std]
use soroban_sdk::{contract, contractimpl};

#[contract]
pub struct Dummy;

#[contractimpl]
impl Dummy {
    pub fn hello() {}
}
