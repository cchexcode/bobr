cargo build --all-features
cargo build --release --all-features
cargo test --all
cargo doc --no-deps --document-private-items --workspace
printf 'a\nb\nc\nd' 1>&2
