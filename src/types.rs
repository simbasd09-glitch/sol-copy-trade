use solana_sdk::signature::Signature;

#[derive(Debug, Clone)]
pub struct TxResult {
    pub signature: Signature,
    pub confirmed: bool,
    pub slot: u64,
}
