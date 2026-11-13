VERSION ?= $(shell sed -n 's/^version = "\([^"]*\)"$$/\1/p' Cargo.toml | head -n 1)
APPS ?= /Applications
INSTALLED = $(APPS)/Ludolph.app/Contents/MacOS/ludolph

# Build Ludolph.app and put it in $(APPS), replacing a running copy and its
# engine. It's signed with a local self-signed identity, so macOS keeps the
# microphone and speech grants across installs. On start, the app starts its
# engine again.
install:
	cargo build --release --locked
	keychain="$$(scripts/local-identity.sh)" && \
		LUDOLPH_SIGN_IDENTITY="Ludolph Local" LUDOLPH_SIGN_KEYCHAIN="$$keychain" \
		scripts/bundle.sh target/release/ludolph $(VERSION) target
	-pkill -f "$(INSTALLED)"
	# The engine gives its pi processes up to 10 s to stop.
	for _ in $$(seq 150); do pgrep -qf "$(INSTALLED)" || break; sleep 0.1; done
	mkdir -p $(APPS)
	rm -rf $(APPS)/Ludolph.app
	cp -R target/Ludolph.app $(APPS)/Ludolph.app
	open $(APPS)/Ludolph.app

test:
	cargo test --workspace --locked

# Run a development engine beside the installed one, with its own files in target/dev.
engine:
	LUDOLPH_DIR=$(CURDIR)/target/dev cargo run --locked -- engine

# Run the windows on the development engine. The installed app keeps the shortcut.
app:
	LUDOLPH_DIR=$(CURDIR)/target/dev cargo run --locked

# Regenerate THIRD_PARTY_LICENSES.md after changing dependencies; CI checks it's current.
licenses:
	cargo about generate --workspace --fail about.hbs > THIRD_PARTY_LICENSES.md

.PHONY: install test engine app licenses
