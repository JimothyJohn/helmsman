# Common dev tasks for Helmsman (Rust, cargo workspace). `make help` lists them.
# Each target is a thin wrapper over a script in ./scripts/ so CI and humans run
# the same thing.

.PHONY: help install-dev lint fmt test-unit test-integration test-all help-site help-site-check help-shots test-interop opener-up opener-down opener-logs rusty-image test-oracle test-plcopen-corpus canvas canvas-web canvas-smoke canvas-serve playground playground-serve node-build node-clippy

help:  ## Show this help.
	@awk 'BEGIN{FS=":.*##"} /^[a-zA-Z_-]+:.*##/ { printf "  \033[1m%-18s\033[0m %s\n", $$1, $$2 }' $(MAKEFILE_LIST)

install-dev:  ## Install the toolchain components the gate needs (rustfmt, clippy).
	./scripts/install-dev.sh

lint:  ## rustfmt check, clippy (warnings denied), the no-box and wasm32 builds.
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

test-interop:  ## Interop suite against HELMSMAN_OPENER_ADDR (default 127.0.0.1, i.e. the container).
	./scripts/test-interop.sh

# ---- Structured Text oracle -------------------------------------------------

rusty-image:  ## Build the pinned ruSTy compiler image (test oracle only, never linked).
	docker build -t helmsman/rusty:1.0.8 docker/rusty

test-oracle:  ## Differential test of the ST interpreter against ruSTy (needs Docker).
	./scripts/test-oracle.sh

test-plcopen-corpus:  ## PLCopen import against Beremiz/OpenPLC Editor example projects (fetched, hash-pinned).
	./scripts/plcopen-corpus.sh

# ---- Canvas -----------------------------------------------------------------

help-site:  ## Build docs/help/*.html from docs/help/*.md plus the generated keyboard and starters pages (needs pandoc).
	./scripts/build-help.sh

help-site-check:  ## Fail if docs/help/ is out of date with the code and the Markdown.
	./scripts/build-help.sh --check

help-shots:  ## Re-take docs/help/img/*.png from target/canvas-dist (make canvas-web first; needs Playwright's Chromium for python3).
	python3 -I scripts/help-shots.py target/canvas-dist docs/help/img

datasheet:  ## Render docs/datasheet.html to the one-page docs/helmsman-datasheet.pdf (needs Chrome or Chromium).
	./scripts/build-datasheet.sh

canvas:  ## Run the canvas (the simulation) as a native window.
	cargo run --release -p helmsman-canvas

canvas-web:  ## Build the canvas for the browser into target/canvas-dist (the box serves it: helmsman-plc --ui).
	./scripts/canvas-web.sh

canvas-smoke:  ## Drive target/canvas-dist in headless Chromium: console errors, panics, a screenshot per step (make canvas-web first; BOX=1 also starts a loopback box).
	python3 -I scripts/canvas-smoke.py $(if $(BOX),--box,)

playground:  ## Build the public ST playground into docs/playground (commit it; Pages serves master's docs/).
	./scripts/build-playground.sh

playground-serve:  ## The playground on 127.0.0.1:8791 with Claude through a local proxy (needs ANTHROPIC_API_KEY).
	./scripts/playground-serve.sh

canvas-serve:  ## Serve the canvas on :8790 (all interfaces) with live rebuild.
	./scripts/canvas-serve.sh

node-build:  ## Build the edge node's firmware: ESP32-POE2 (esp toolchain) or NODE_BOARD=p4-eth (docs/EDGE.md).
	./scripts/node-build.sh

node-clippy:  ## Clippy the edge node's firmware with warnings denied.
	./scripts/node-build.sh clippy
