cargo build --all-features
cargo build --release --all-features
cargo test --all --all-features -- --test-threads=1
cargo doc --no-deps --document-private-items --workspace
