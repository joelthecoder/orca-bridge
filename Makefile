MISE ?= env MISE_AUTO_INSTALL=false mise exec --
CARGO ?= $(MISE) cargo
ARGS ?=
INSTALL_DIR ?= $(HOME)/.local/bin
.DEFAULT_GOAL := help
.PHONY: help setup doctor all build format lint test check run install uninstall clean smoke smoke-native profile artwork alfred raycast url-install

##@ Development
help: ## Show available commands
	@sh scripts/make-help.sh $(MAKEFILE_LIST)
setup: ## Install the pinned Rust toolchain and fetch dependencies
	mise install
	$(CARGO) fetch --locked
doctor: ## Inspect Rust and local Orca compatibility
	$(CARGO) --version
	$(CARGO) run --locked -- doctor
all: check build ## Check and build everything
build: ## Build the optimized CLI
	$(CARGO) build --locked --release
format: ## Format Rust source files
	$(CARGO) fmt --all
run: ## Run the CLI with ARGS
	$(CARGO) run --locked -- $(ARGS)

##@ Verification
lint: ## Check formatting and compiler lints
	$(CARGO) fmt --all -- --check
	$(CARGO) clippy --locked --all-targets -- -D warnings
test: ## Run deterministic behavioral tests
	$(CARGO) test --locked
check: lint test ## Run all local checks
smoke: build ## Exercise read-only commands against running Orca
	python3 scripts/smoke.py target/release/orca-bridge

smoke-native: build ## Verify native snapshots and a disposable terminal create/remove event
	python3 scripts/smoke-native.py target/release/orca-bridge --exercise
profile: build ## Measure process startup, history load and search ranking
	$(CARGO) run --locked --release --example profile-search
	python3 scripts/benchmark.py target/release/orca-bridge

##@ Launchers
artwork: ## Regenerate transparent README logos for light and dark themes
	python3 scripts/build-readme-logo.py
alfred: ## Package the importable Alfred workflow
	python3 scripts/package-alfred.py
raycast: ## Build and typecheck the local Raycast extension
	cd integrations/raycast && npm ci --ignore-scripts && npm run lint && npm run build
url-install: build ## Install and register the macOS session URL handler
	python3 scripts/install-url-handler.py

##@ Installation
install: build ## Install the CLI in INSTALL_DIR
	install -d "$(INSTALL_DIR)"
	install -m 755 target/release/orca-bridge "$(INSTALL_DIR)/orca-bridge"
uninstall: ## Remove the installed CLI
	rm -f "$(INSTALL_DIR)/orca-bridge"
clean: ## Remove Cargo build artifacts
	$(CARGO) clean
