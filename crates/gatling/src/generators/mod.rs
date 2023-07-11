use rand::Rng;
use starknet::{
    core::types::FieldElement,
    signers::{LocalWallet, SigningKey},
};
// use starknet::{
//     accounts::{Account, Call, SingleOwnerAccount},
//     core::{
//         chain_id,
//         types::{BlockId, BlockTag, FieldElement},
//         utils::get_selector_from_name,
//     },
//     providers::SequencerGatewayProvider,
//     signers::{LocalWallet, SigningKey},
// };

const _TESTNET_TOKEN_ADDRESS: &str = "07394cbe418daa16e42b87ba67372d4ab4a5df0b05c6e554d158458ce245bc10";

/// generate random number for testing
pub fn get_rng() -> FieldElement {
    let mut rng = rand::thread_rng();
    FieldElement::from(rng.gen::<u64>())
}


pub fn generate_stark_keys() -> LocalWallet {
    let private = get_rng();

    LocalWallet::from(SigningKey::from_secret_scalar(private))
}

// pub fn new_account(_seed: u64) -> SingleOwnerAccount {
//     let private = get_rng();
//     let address_seed = get_rng();

//     let signer = LocalWallet::from(SigningKey::from_secret_scalar(private));

//     // calculate contract address
//     let address = FieldElement::from_hex_be("FROM ADDRESS CALCULATION").unwrap();
//     let tst_token_address = FieldElement::from_hex_be(TESTNET_TOKEN_ADDRESS).unwrap();

//     // TODO: swap for JSON-RPC provider we are testing
//     let provider = SequencerGatewayProvider::starknet_alpha_goerli();

//     SingleOwnerAccount::new(provider, signer, address, chain_id::TESTNET)
// }
