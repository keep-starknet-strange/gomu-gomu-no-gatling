use rand::Rng;

use starknet::{
    core::types::FieldElement,
    signers::{LocalWallet, SigningKey},
};

/// generate random number for testing
pub fn get_rng() -> FieldElement {
    let mut rng = rand::thread_rng();
    FieldElement::from(rng.gen::<u64>())
}

pub fn generate_stark_keys() -> LocalWallet {
    let private = get_rng();

    LocalWallet::from(SigningKey::from_secret_scalar(private))
}
