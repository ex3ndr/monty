.DEFAULT_GOAL := all

.PHONY: .cargo
.cargo: ## Check that cargo is installed
	@cargo --version || echo 'Please install cargo: https://github.com/rust-lang/cargo'

.PHONY: .pre-commit
.pre-commit: ## Check that pre-commit is installed
	@pre-commit -V || echo 'Please install pre-commit: https://pre-commit.com/'

.PHONY: install
install: .cargo .pre-commit ## Install the package, dependencies, and pre-commit for local development
	cargo check
	pre-commit install --install-hooks

.PHONY: lint
lint:
	@cargo fmt --version
	cargo fmt --all -- --check
	@cargo clippy --version
	cargo clippy --tests -- -D warnings -A incomplete_features -W clippy::dbg_macro

.PHONY: test
test:
	cargo test

.PHONY: all
all: lint test
