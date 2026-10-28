# Common dev tasks for tcpethernet (Rust, cargo workspace). `make help` lists them.
# Each target is a thin wrapper over a script in ./scripts/ so CI and humans run
# the same thing.

.PHONY: help install-dev lint fmt test-unit test-integration test-all test-interop opener-up opener-down opener-logs rusty-image test-oracle

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

# ---- Interop targets --------------------------------------------------------

opener-up:  ## Build and start the OpENer adapter container (ports 44818 tcp/udp, 2222 udp).
	docker compose up -d --build opener

opener-down:  ## Stop and remove the OpENer container.
	docker compose down

opener-logs:  ## Tail the OpENer container.
	docker compose logs -f opener

test-interop:  ## Interop suite against TCPETHERNET_OPENER_ADDR (default 127.0.0.1, i.e. the container).
	./scripts/test-interop.sh

# ---- Structured Text oracle -------------------------------------------------

rusty-image:  ## Build the pinned ruSTy compiler image (test oracle only, never linked).
	docker build -t tcpethernet/rusty:1.0.7 docker/rusty

test-oracle:  ## Differential test of the ST interpreter against ruSTy (needs Docker).
	./scripts/test-oracle.sh
