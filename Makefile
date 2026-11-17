VERSION ?= $(shell sed -n 's/^version = "\([^"]*\)"$$/\1/p' Cargo.toml | head -n 1)
APPS ?= /Applications
INSTALLED = $(APPS)/Ludolph.app/Contents/MacOS/ludolph

install:
	cargo build --release --locked
	keychain="$$(scripts/local-identity.sh)" && \
		LUDOLPH_SIGN_IDENTITY="Ludolph Local" LUDOLPH_SIGN_KEYCHAIN="$$keychain" \
		scripts/bundle.sh target/release/ludolph $(VERSION) target
	-pkill -f "$(INSTALLED)"
	for _ in $$(seq 150); do pgrep -qf "$(INSTALLED)" || break; sleep 0.1; done
	mkdir -p $(APPS)
	rm -rf $(APPS)/Ludolph.app
	cp -R target/Ludolph.app $(APPS)/Ludolph.app
	open $(APPS)/Ludolph.app

test:
	cargo test --workspace --locked
	cd extensions && bun install --frozen-lockfile && bun run typecheck && bun test

# Development engines load the extensions from target/pi.
extensions:
	cd extensions && bun install --frozen-lockfile && bun run build $(CURDIR)/target/pi

engine: extensions
	LUDOLPH_DIR=$(CURDIR)/target/dev cargo run --locked -- engine

app: extensions
	LUDOLPH_DIR=$(CURDIR)/target/dev cargo run --locked

licenses:
	cargo about generate --workspace --fail about.hbs > THIRD_PARTY_LICENSES.md

.PHONY: install test extensions engine app licenses
