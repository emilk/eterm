#!/bin/bash
script_path=$( cd "$(dirname "${BASH_SOURCE[0]}")" ; pwd -P )
cd "$script_path"
set -eux

# Checks all tests, lints etc.
# Basically does what the CI does.

cargo check --workspace --all-targets --all-features
cargo test --workspace --all-targets --all-features
cargo clippy --workspace --all-targets --all-features --  -D warnings -W clippy::all
cargo fmt --all -- --check

cargo doc -p eterm --lib --no-deps --all-features
