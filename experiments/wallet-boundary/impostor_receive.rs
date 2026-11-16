// Reuse the saved real-GUI-backend impostor fixture; run only the new test.
include!("../wallet-vs-node/harness/tests/deceive.rs");

#[tokio::test]
async fn receive_address_can_be_replaced_without_the_wallet_secret() {
    use noid_poseidon2b::primitives::Address;

    // These are valid, independently generated synthetic addresses. The first
    // belongs to the victim fixture, the second to a different known secret.
    let owner = "o186k54fnkd0ssd25pd7yhgzgtnewe9rvj857vrl3lks2kgppavvgq2ymvua";
    let attacker = "o1uqppny90hhl7e5gyvnxgh2dj60gddehxr7l97ue04au60qfszxdsmk8x3t";
    let owner_decoded = Address::parse(owner).expect("valid victim address");
    let attacker_decoded = Address::parse(attacker).expect("valid attacker address");
    assert_ne!(owner_decoded, attacker_decoded);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let impostor = Impostor::new();
    tokio::spawn(serve(listener, Arc::clone(&impostor)));
    let data_dir = tempfile::tempdir().unwrap();
    std::env::set_var("NOID_RPC", format!("http://127.0.0.1:{port}"));
    std::env::set_var("NOID_GUI_DATA_DIR", data_dir.path());
    std::env::set_var("NOID_GUI_SETTINGS_FILE", data_dir.path().join("none.json"));
    std::env::remove_var("NOID_GUI_MOCK");
    let backend = Backend::from_env();

    script_baseline(&impostor, 4_321, 0, 0);
    impostor.set("walletListAddresses", json!([{"address": owner, "key_index": 0, "is_active": true}]));
    impostor.set("walletActiveAddress", json!({"address": owner, "key_index": 0, "is_active": true}));
    let normal = backend.snapshot().await.unwrap();
    assert_eq!(normal.snapshot.active_address().address, owner);

    impostor.set("walletListAddresses", json!([{"address": attacker, "key_index": 0, "is_active": true}]));
    impostor.set("walletActiveAddress", json!({"address": attacker, "key_index": 0, "is_active": true}));
    let poisoned = backend.snapshot().await.unwrap();
    assert_eq!(poisoned.snapshot.active_address().address, attacker);
    assert_eq!(poisoned.snapshot.addresses[0].address, attacker);
    assert!(impostor.call_count("walletListAddresses") >= 2);
    assert!(impostor.call_count("walletActiveAddress") >= 2);
    println!("normal_receive={owner}");
    println!("poisoned_receive={attacker}");
    println!("PASS: the real GUI backend accepts an attacker's valid address as its active receive address");
}
