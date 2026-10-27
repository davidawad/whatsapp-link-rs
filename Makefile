.PHONY: install ci fmt fmt-check lint typecheck test audit run build clean

# Full fast gate - pre-commit and CI both invoke this
ci: fmt-check lint typecheck test

install:
	cargo fetch --locked
	pre-commit install --install-hooks --hook-type pre-commit --hook-type commit-msg

fmt:
	cargo fmt

fmt-check:
	cargo fmt --check

lint:
	cargo clippy --all-targets -- -D warnings

typecheck:
	cargo check --all-targets --locked

test:
	cargo test --locked

audit:
	cargo audit

run:
	cargo run --locked -- --help

build:
	cargo build --release --locked

clean:
	cargo clean
