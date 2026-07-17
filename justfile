# Recipes never set a target dir; set CARGO_TARGET_DIR to build
# somewhere other than the default target/.

# list available recipes
default:
    @just --list

# debug build
build:
    cargo build

# release build
release:
    cargo build --release

# run the test suite
test:
    cargo test

# clippy, warnings fail the gate
lint:
    cargo clippy -- -D warnings

# install the issues CLI
install:
    cargo install --path issues
