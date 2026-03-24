.PHONY: all fmt fmt-check lint test check build release clean

all: fmt lint test

## Format all code
fmt:
	cargo fmt --all

## Check formatting without modifying files
fmt-check:
	cargo fmt --all -- --check

## Run clippy lints (clean first to avoid cross-toolchain artifact issues)
lint:
	cargo clippy --workspace --all-targets -- -D warnings

## Run all tests
test:
	cargo test --workspace

## Format check + lint + test (CI-style check)
check: fmt-check lint test

## Build the project
build:
	cargo build

## Build in release mode
release:
	cargo build --release

## Remove build artifacts
clean:
	cargo clean
