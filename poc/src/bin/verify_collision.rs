use noid_poseidon2b::native::compression::compress_flat_feed_forward_with_tag;
use noid_poseidon2b::native::domain::TAG_CAPSNODE;

fn input(left_hex: &str, right_hex: &str) -> ([u8; 32], [u8; 32]) {
    (
        hex::decode(left_hex).unwrap().try_into().unwrap(),
        hex::decode(right_hex).unwrap().try_into().unwrap(),
    )
}

fn main() {
    let first = input(
        "2d8a16372955da0403d2a69740bd860cb83fa1c69c1e7129c671a616a5951930",
        "e681a0b3bee99f85353fbaac20999ef2ec23409e03068aa9e97759648e114d01",
    );
    let second = input(
        "6f7bb24eb68869bdb4f59a582a6a984c5e40eaa3ade728c02e887b0303825ca6",
        "e6312b07507b9f4cc0675c2f9fcd656d4e90a4a753c0a5314779d2d3896cf208",
    );

    assert_ne!(first, second);
    let first_digest = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &first.0, &first.1);
    let second_digest = compress_flat_feed_forward_with_tag(TAG_CAPSNODE, &second.0, &second.1);
    assert_eq!(first_digest, second_digest);

    println!("first:  {}", hex::encode(first_digest));
    println!("second: {}", hex::encode(second_digest));
}
