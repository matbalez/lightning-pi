use bitcoin::{
    hashes::{Hash, sha256},
    secp256k1::{PublicKey, Secp256k1, SecretKey},
};
use lightning_invoice::{Currency, InvoiceBuilder, PaymentHash, PaymentSecret};
use lightning_pi::{
    protocol as p,
    server::{self, Config, Receiver},
    store::ReplayStore,
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

struct Mock {
    key: SecretKey,
    pk: String,
    seq: AtomicU64,
    proofs: Mutex<HashMap<String, String>>,
}
#[async_trait::async_trait]
impl Receiver for Mock {
    fn pay_to(&self) -> &str {
        &self.pk
    }
    async fn invoice(&self, amount: u64, hash: [u8; 32], expiry: u32) -> anyhow::Result<String> {
        let count = self.seq.fetch_add(1, Ordering::SeqCst);
        let pre = sha256::Hash::hash(&count.to_be_bytes()).to_byte_array();
        let invoice = InvoiceBuilder::new(Currency::Bitcoin)
            .amount_milli_satoshis(amount)
            .description_hash(sha256::Hash::from_byte_array(hash))
            .payment_hash(PaymentHash(sha256::Hash::hash(&pre).to_byte_array()))
            .payment_secret(PaymentSecret([7; 32]))
            .duration_since_epoch(Duration::from_secs(p::now()))
            .expiry_time(Duration::from_secs(expiry.into()))
            .min_final_cltv_expiry_delta(18)
            .build_signed(|m| Secp256k1::new().sign_ecdsa_recoverable(m, &self.key))?
            .to_string();
        self.proofs
            .lock()
            .unwrap()
            .insert(invoice.clone(), hex::encode(pre));
        Ok(invoice)
    }
}
#[tokio::test]
async fn full_http_flow_and_concurrent_replay() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(ReplayStore::open(&dir.path().join("replay.sqlite")).unwrap());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let settle = format!("http://{}/settle", listener.local_addr().unwrap());
    let facilitator = tokio::spawn(async move {
        axum::serve(listener, server::facilitator(store))
            .await
            .unwrap()
    });
    let key = SecretKey::from_slice(&[3; 32]).unwrap();
    let mock = Arc::new(Mock {
        pk: PublicKey::from_secret_key(&Secp256k1::new(), &key).to_string(),
        key,
        seq: AtomicU64::new(0),
        proofs: Mutex::new(HashMap::new()),
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    let app = server::app(
        Config::new(origin.clone(), 100000, 10000).unwrap(),
        mock.clone(),
        settle,
    );
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let http = reqwest::Client::new();
    let url = format!("{origin}/digits-of-pi?digits=3");
    for q in [
        "digits=-1",
        "digits=10001",
        "digits=3&digits=4",
        "digits=01",
        "digits=1.0",
        "digits=%31",
        "digits=3&x=1",
        "",
    ] {
        assert_eq!(
            http.get(format!("{origin}/digits-of-pi?{q}"))
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    assert_eq!(mock.seq.load(Ordering::SeqCst), 0);
    assert_eq!(
        http.get(&url)
            .header("Host", "attacker.invalid")
            .send()
            .await
            .unwrap()
            .status(),
        400
    );
    assert_eq!(http.head(&url).send().await.unwrap().status(), 405);
    let r = http.get(&url).send().await.unwrap();
    assert_eq!(r.status(), 402);
    let challenge = p::decode(r.headers()["payment-required"].to_str().unwrap()).unwrap();
    assert_eq!(r.json::<Value>().await.unwrap(), challenge);
    let accepted = &challenge["accepts"][0];
    let invoice = accepted["extra"]["invoice"].as_str().unwrap();
    let pre = mock.proofs.lock().unwrap()[invoice].clone();
    let payload = json!({"x402Version":2,"accepted":accepted,"payload":{"preimage":pre}});
    let signature = p::encode(&payload);
    let wrong = http
        .get(format!("{origin}/digits-of-pi?digits=4"))
        .header("payment-signature", &signature)
        .send()
        .await
        .unwrap();
    assert_eq!(wrong.status(), 402);
    assert_eq!(
        wrong.json::<Value>().await.unwrap()["error"],
        "invalid_exact_lnbtc_request_mismatch"
    );
    let mut forged = payload.clone();
    forged["payload"]["preimage"] = json!("00".repeat(32));
    assert_eq!(
        http.get(&url)
            .header("payment-signature", p::encode(&forged))
            .send()
            .await
            .unwrap()
            .status(),
        402
    );
    let (a, b) = tokio::join!(
        http.get(&url)
            .header("payment-signature", &signature)
            .send(),
        http.get(&url)
            .header("payment-signature", &signature)
            .send()
    );
    let (a, b) = (a.unwrap(), b.unwrap());
    assert!((a.status() == 200 && b.status() == 409) || (a.status() == 409 && b.status() == 200));
    let good = if a.status() == 200 { a } else { b };
    let receipt = p::decode(good.headers()["payment-response"].to_str().unwrap()).unwrap();
    assert_eq!(receipt["success"], true);
    assert!(receipt.get("payer").is_none());
    assert_eq!(good.json::<Value>().await.unwrap()["pi"], "3.142");
    assert_eq!(
        mock.seq.load(Ordering::SeqCst),
        1,
        "Paid retries do not create new invoices"
    );
    // Exercise the actual CLI's wallet-independent workflow and durable state.
    let state = dir.path().join("purchase.json");
    let client_url = format!("{origin}/digits-of-pi?digits=10");
    let bin = env!("CARGO_BIN_EXE_x402-client");
    let prepared = tokio::process::Command::new(bin)
        .arg("--state")
        .arg(&state)
        .args(["prepare", "--url", &client_url])
        .output()
        .await
        .unwrap();
    assert!(
        prepared.status.success(),
        "{}",
        String::from_utf8_lossy(&prepared.stderr)
    );
    let prepared: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    let client_invoice = prepared["invoice"].as_str().unwrap();
    let pre = mock.proofs.lock().unwrap()[client_invoice].clone();
    let mut result = json!({"status":"paid","invoice":client_invoice,"paymentHash":prepared["paymentHash"],"amountMsat":"1","feeMsat":"0","preimage":pre});
    let result_path = dir.path().join("wallet-result.json");
    std::fs::write(&result_path, serde_json::to_vec(&result).unwrap()).unwrap();
    let bad = tokio::process::Command::new(bin)
        .arg("--state")
        .arg(&state)
        .args(["attach-result", "--payment-result"])
        .arg(&result_path)
        .output()
        .await
        .unwrap();
    assert!(
        !bad.status.success(),
        "Must reject a wallet result for the wrong amount"
    );
    result["amountMsat"] = json!("100000");
    std::fs::write(&result_path, serde_json::to_vec(&result).unwrap()).unwrap();
    let attached = tokio::process::Command::new(bin)
        .arg("--state")
        .arg(&state)
        .args(["attach-result", "--payment-result"])
        .arg(&result_path)
        .output()
        .await
        .unwrap();
    assert!(
        attached.status.success(),
        "{}",
        String::from_utf8_lossy(&attached.stderr)
    );
    for _ in 0..2 {
        let redeemed = tokio::process::Command::new(bin)
            .arg("--state")
            .arg(&state)
            .arg("redeem")
            .output()
            .await
            .unwrap();
        assert!(
            redeemed.status.success(),
            "{}",
            String::from_utf8_lossy(&redeemed.stderr)
        );
        let value: Value = serde_json::from_slice(&redeemed.stdout).unwrap();
        assert_eq!(value["pi"], "3.1415926536");
    }
    let repeat = tokio::process::Command::new(bin)
        .arg("--state")
        .arg(&state)
        .args(["prepare", "--url", &client_url])
        .output()
        .await
        .unwrap();
    assert!(!repeat.status.success());
    assert_eq!(mock.seq.load(Ordering::SeqCst), 2);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&state).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let mdk_state = dir.path().join("mdk-purchase.json");
    let prepared = tokio::process::Command::new(bin)
        .arg("--state")
        .arg(&mdk_state)
        .args([
            "prepare",
            "--url",
            &format!("{origin}/digits-of-pi?digits=4"),
        ])
        .output()
        .await
        .unwrap();
    assert!(prepared.status.success());
    let prepared: Value = serde_json::from_slice(&prepared.stdout).unwrap();
    let invoice = prepared["invoice"].as_str().unwrap();
    let history = json!({"payments":[{"destination":invoice,"direction":"outbound","status":"completed","amountSats":100,"paymentHash":prepared["paymentHash"],"preimage":mock.proofs.lock().unwrap()[invoice]}]});
    std::fs::write(&result_path, serde_json::to_vec(&history).unwrap()).unwrap();
    let attached = tokio::process::Command::new(bin)
        .arg("--state")
        .arg(&mdk_state)
        .args(["attach-mdk", "--payments-file"])
        .arg(&result_path)
        .output()
        .await
        .unwrap();
    assert!(
        attached.status.success(),
        "{}",
        String::from_utf8_lossy(&attached.stderr)
    );
    let redeemed = tokio::process::Command::new(bin)
        .arg("--state")
        .arg(&mdk_state)
        .arg("redeem")
        .output()
        .await
        .unwrap();
    assert!(redeemed.status.success());
    let result: Value = serde_json::from_slice(&redeemed.stdout).unwrap();
    assert_eq!(result["pi"], "3.1416");
    task.abort();
    facilitator.abort();
}
