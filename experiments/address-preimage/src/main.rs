mod field128;
mod gf8;

fn main() {
    let command = std::env::args().nth(1).unwrap_or_else(|| "all".to_string());
    match command.as_str() {
        "structure" => field128::structure_experiment(),
        "weak-key" => field128::weak_key_experiment(),
        "reduced-field" => gf8::reduced_field_experiment(),
        "all" => {
            field128::structure_experiment();
            field128::weak_key_experiment();
            gf8::reduced_field_experiment();
        }
        _ => {
            eprintln!("usage: cargo run --release -- [structure|weak-key|reduced-field|all]");
            std::process::exit(2);
        }
    }
}
