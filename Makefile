.DEFAULT_GOAL := help

.PHONY: help fmt-check clippy test build release

help:
	@echo "Available targets:"
	@echo "  make fmt-check  Check Rust formatting"
	@echo "  make clippy     Run Clippy with warnings denied"
	@echo "  make test       Run the locked test suite"
	@echo "  make build      Build a locked release binary"
	@echo "  make release    Run all local release checks"

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --locked --all-targets --all-features -- -D warnings

test:
	cargo test --locked --all-features

build:
	cargo build --release --locked

release: fmt-check clippy test build
	@echo "Release checks passed for joocode v$$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)."
