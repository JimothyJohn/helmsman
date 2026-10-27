# Common dev tasks for tcpethernet (Rust, cargo workspace). `make help` lists them.
# Each target is a thin wrapper over a script in ./scripts/ so CI and humans run
# the same thing.

.PHONY: help install-dev lint fmt test-unit test-integration test-all

help:  ## Show this help.
	@awk 'BEGIN{FS=":.*##"} /^[a-zA-Z_-]+:.*##/ { printf "  \033[1m%-18s\033[0m %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

install-dev:  ## Install the toolchain components the gate needs (rustfmt, clippy).
	./scripts/install-dev.sh

lint:  ## rustfmt check + clippy with warnings denied.
	./scripts/lint.sh

fmt:  ## Format the workspace in place.
	./scripts/fmt.sh

test-unit:  ## Library unit tests only.
	./scripts/test-unit.sh

test-integration:  ## Integration tests (property tests, loopback UDP).
	./scripts/test-integration.sh

test-all:  ## Everything, locked to Cargo.lock.
	./scripts/test-all.sh
