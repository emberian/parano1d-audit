//! Offline corruption helper for the devnet-chaos lane.
//!
//! Applies single-row MDBX edits to a stopped node's `<data-dir>/mdbx.dat`
//! using production-identical MDBX options (technique identical to the
//! storage-reorg lane's second-writer corruption).
//!
//! Modes:
//!   supply-up <delta-u128>   add delta to state_meta[0][20..36] (LE u128)
//!   supply-zero              zero state_meta[0][20..36]
//!   tip-append               append 0xA5 to tip[0] (41-byte noncanonical chain_tip)
//!   read                     dump state_meta[0] and tip[0] lengths/values (control)

use std::path::Path;

use anyhow::{bail, Context, Result};
use libmdbx::{Database, DatabaseOptions, Mode, NoWriteMap, ReadWriteOptions, SyncMode, WriteFlags};

fn open_raw(path: &Path) -> Result<Database<NoWriteMap>> {
    let rw = ReadWriteOptions {
        sync_mode: SyncMode::Durable,
        min_size: Some(4 * 1024 * 1024),
        max_size: Some(1024isize * 1024 * 1024 * 1024),
        growth_step: Some(64 * 1024 * 1024),
        ..Default::default()
    };
    Ok(Database::<NoWriteMap>::open_with_options(
        path,
        DatabaseOptions {
            max_tables: Some(24),
            mode: Mode::ReadWrite(rw),
            liforeclaim: true,
            rp_augment_limit: Some(65_536),
            ..Default::default()
        },
    )?)
}

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 3 {
        bail!(
            "usage: corrupt-db <data-dir>/mdbx.dat <supply-up|supply-zero|tip-append|read> [delta]"
        );
    }
    let db_path = std::path::PathBuf::from(&args[1]);
    let mode = args[2].as_str();
    let db = open_raw(&db_path)?;
    let txn = db.begin_rw_txn()?;

    match mode {
        "read" => {
            let meta = txn.open_table(Some("state_meta"))?;
            let raw: Vec<u8> = txn.get(&meta, &[0u8][..])?.context("state_meta[0] missing")?;
            let supply = if raw.len() >= 36 {
                u128::from_le_bytes(raw[20..36].try_into().unwrap())
            } else {
                0
            };
            let tip = txn.open_table(Some("tip"))?;
            let tip_raw: Option<Vec<u8>> = txn.get(&tip, &[0u8][..]).ok().flatten();
            println!(
                "state_meta[0] len={} supply={} tip[0] len={}",
                raw.len(),
                supply,
                tip_raw.as_ref().map_or(0, Vec::len)
            );
            return Ok(());
        }
        "supply-up" | "supply-zero" => {
            let table = txn.open_table(Some("state_meta"))?;
            let mut raw: Vec<u8> = txn.get(&table, &[0u8][..])?.context("state_meta[0] missing")?;
            if raw.len() != 36 {
                bail!("state_meta[0] has unexpected length {}", raw.len());
            }
            let current = u128::from_le_bytes(raw[20..36].try_into().unwrap());
            let next = if mode == "supply-zero" {
                0u128
            } else {
                let delta: u128 = args
                    .get(3)
                    .context("supply-up requires a delta argument")?
                    .parse()
                    .context("delta must parse as u128")?;
                current
                    .checked_add(delta)
                    .context("supply overflow")?
            };
            raw[20..36].copy_from_slice(&next.to_le_bytes());
            txn.put(&table, &[0u8][..], &raw, WriteFlags::empty())?;
            txn.commit()?;
            println!("corrupt-db: supply {current} -> {next}");
        }
        "tip-append" => {
            let table = txn.open_table(Some("tip"))?;
            let mut raw: Vec<u8> = txn.get(&table, &[0u8][..])?.context("tip[0] missing")?;
            if raw.len() != 40 {
                bail!("tip[0] has unexpected length {} (want 40)", raw.len());
            }
            raw.push(0xA5);
            txn.put(&table, &[0u8][..], &raw, WriteFlags::empty())?;
            txn.commit()?;
            println!("corrupt-db: chain_tip 40 -> 41 bytes");
        }
        other => bail!("unknown mode {other}"),
    }
    Ok(())
}
