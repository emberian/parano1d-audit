// Address-level control for photo_context_diff.py. Takes two synthetic key hexes.
use std::path::PathBuf;
use wallet_safety_repro::wallet::state::{import_generated_master_secret, WalletState};

fn main() {
    let mut args = std::env::args().skip(1);
    for (index, label) in ["shipped", "documented"].into_iter().enumerate() {
        let secret = args.next().expect("two synthetic keys required");
        let directory = PathBuf::from("experiments/wallet-boundary/target")
            .join(format!("address-case-{index}"));
        std::fs::create_dir_all(&directory).unwrap();
        let key_path = directory.join("wallet.key");
        import_generated_master_secret(&key_path, &secret).unwrap();
        let wallet = WalletState::create_or_load(key_path).unwrap();
        println!("{label}_address={}", wallet.active_address().to_bech32());
    }
}
