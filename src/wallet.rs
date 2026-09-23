use anyhow::{Context, Result};
use lexe::{
    config::WalletEnvConfig,
    types::{
        auth::{ClientCredentials, CredentialsRef},
        bitcoin::Amount,
    },
    wallet::LexeWallet,
};
use lexe_api_core::{def::UserNodeRunApi, models::command::CreateInvoiceRequest};

pub fn load_wallet() -> Result<LexeWallet> {
    let secret = match std::env::var("LEXE_CLIENT_CREDENTIALS_PATH") {
        Ok(path) => std::fs::read_to_string(path).context("Read credential file")?,
        Err(_) => std::env::var("LEXE_CLIENT_CREDENTIALS")
            .context("Set LEXE_CLIENT_CREDENTIALS or LEXE_CLIENT_CREDENTIALS_PATH")?,
    };
    let credentials = ClientCredentials::from_string(secret.trim())?;
    LexeWallet::without_db(
        WalletEnvConfig::mainnet(),
        CredentialsRef::from(&credentials),
    )
}

pub async fn create_bound_invoice(
    wallet: &LexeWallet,
    amount_msat: u64,
    hash: [u8; 32],
    expiry: u32,
) -> Result<String> {
    // The stable SDK currently drops description_hash. Keep this version-pinned
    // lower-level call isolated until Lexe exposes it in CreateInvoiceRequest.
    let result = wallet
        .node_client()
        .create_invoice(CreateInvoiceRequest {
            expiration_secs: expiry,
            amount: Some(Amount::from_msat(amount_msat)),
            description: None,
            description_hash: Some(hash),
            ..Default::default()
        })
        .await?;
    Ok(result.invoice.to_string())
}
