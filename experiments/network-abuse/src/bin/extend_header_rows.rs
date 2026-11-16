//! Offline serving-path fixture. The inserted headers are syntactically
//! decodable but deliberately *not* a valid consensus chain. Never use this
//! store to test chain acceptance or fork choice.
use std::path::Path;

use anyhow::{bail, Context, Result};
use libmdbx::{
    Database, DatabaseOptions, Mode, NoWriteMap, ReadWriteOptions, SyncMode, WriteFlags,
};

fn randomize_header(mut raw: Vec<u8>, height: u64) -> Vec<u8> {
    // Preserve the original miner address and scalar canonical encodings.
    // The first three 32-byte digests are arbitrary bytes in the wire grammar.
    let mut state = height ^ 0x1fd2_e3a4_c5b6_7980;
    for byte in &mut raw[..96] {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    raw[104..112].copy_from_slice(&height.to_le_bytes());
    raw
}

fn main() -> Result<()> {
    let arg = std::env::args()
        .nth(1)
        .context("usage: extend_header_rows <stopped-copied-data-dir>/mdbx.dat")?;
    let path = Path::new(&arg);
    if !path
        .to_string_lossy()
        .contains("/experiments/network-abuse/run/")
    {
        bail!("refusing path outside this lane's ignored run directory");
    }
    let rw = ReadWriteOptions {
        sync_mode: SyncMode::Durable,
        min_size: Some(4 * 1024 * 1024),
        max_size: Some(1024isize * 1024 * 1024 * 1024),
        growth_step: Some(64 * 1024 * 1024),
        ..Default::default()
    };
    let db = Database::<NoWriteMap>::open_with_options(
        path,
        DatabaseOptions {
            max_tables: Some(24),
            mode: Mode::ReadWrite(rw),
            liforeclaim: true,
            rp_augment_limit: Some(65_536),
            ..Default::default()
        },
    )?;
    let txn = db.begin_rw_txn()?;
    let table = txn.open_table(Some("headers"))?;
    let raw: Vec<u8> = txn
        .get(&table, &150u64.to_le_bytes())?
        .context("height 150 missing")?;
    if raw.len() != 212 {
        bail!("unexpected header wire length {}", raw.len());
    }
    for height in 151..=4096u64 {
        let header = randomize_header(raw.clone(), height);
        txn.put(&table, &height.to_le_bytes(), &header, WriteFlags::empty())?;
    }
    txn.commit()?;
    println!(
        "inserted 3946 syntactically encoded, consensus-invalid header rows at heights 151..4096"
    );
    Ok(())
}
