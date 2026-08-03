.DEFAULT_GOAL := help

.PHONY: help fmt-check clippy test build release

help:
	@echo "Available targets:"
	@echo "  make fmt-check  Check Rust formatting"
	@echo "  make clippy     Run Clippy with warnings denied"
	@echo "  make test       Run the locked test suite"
	@echo "  make build      Build a locked release binary"
	@echo "  make release    Bump version, validate, commit, tag, and push a release"
	@echo "                  Optional: BUMP=minor|major, VERSION=x.y.z, DRY_RUN=1"

fmt-check:
	cargo fmt --all --check

clippy:
	cargo clippy --locked --all-targets --all-features -- -D warnings

test:
	cargo test --locked --all-features

build:
	cargo build --release --locked

release:
	@./scripts/release.sh
