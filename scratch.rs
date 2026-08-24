use soroban_sdk::{Env, BytesN};

fn main() {
    let env = Env::default();
    let wasm = &[0x00, 0x61, 0x73, 0x6d, 0x01, 0x00, 0x00, 0x00];
    let hash = env.deployer().upload_contract_wasm(wasm);
    println!("Hash: {:?}", hash);
}
