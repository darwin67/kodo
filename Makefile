.PHONY: all fmt fmt-check lint test check build release clean reset

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

## Reset all stored auth and session data (dev convenience)
reset:
	@echo "Removing session state..."
	@rm -f ~/.config/kodo/last_session.json
	@echo "Removing stored tokens from keychain..."
	@security delete-generic-password -s kodo -a kodo-anthropic 2>/dev/null || true
	@security delete-generic-password -s kodo -a kodo-openai 2>/dev/null || true
	@security delete-generic-password -s kodo -a kodo-gemini 2>/dev/null || true
	@echo "Done. All kodo auth data cleared."
