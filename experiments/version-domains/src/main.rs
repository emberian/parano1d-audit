mod authorization;
mod network_profile;
mod proofs;
mod snapshot;
mod terminal;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mode = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());
    match mode.as_str() {
        "network-profile" => network_profile::run(),
        "authorization" => authorization::run(),
        "proofs" => proofs::run(),
        "terminal" => terminal::run(),
        "snapshot" => snapshot::run(),
        "all" => {
            network_profile::run()?;
            authorization::run()?;
            proofs::run()?;
            terminal::run()?;
            snapshot::run()
        }
        _ => Err(format!("unknown mode {mode}").into()),
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(2 * bytes.len());
    for &byte in bytes {
        out.push(DIGITS[(byte >> 4) as usize] as char);
        out.push(DIGITS[(byte & 15) as usize] as char);
    }
    out
}
