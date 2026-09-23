use anyhow::Result;
use lightning_pi::wallet::{create_bound_invoice, load_wallet};

#[tokio::main]
async fn main() -> Result<()> {
    let wallet = load_wallet()?;
    let node = wallet.node_info().await?;
    let invoice = create_bound_invoice(&wallet, 100_000, [42; 32], 300).await?;
    println!(
        "{}",
        serde_json::json!({"node_pk":node.node_pk.to_string(),"invoice":invoice})
    );
    Ok(())
}
