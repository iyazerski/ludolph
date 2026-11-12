test:
	cargo test --workspace --locked

# Run a development engine beside the installed one, with its own files in target/dev.
engine:
	LUDOLPH_DIR=$(CURDIR)/target/dev cargo run --locked -- engine

# Regenerate THIRD_PARTY_LICENSES.md after changing dependencies; CI checks it's current.
licenses:
	cargo about generate --workspace --fail about.hbs > THIRD_PARTY_LICENSES.md

.PHONY: test engine licenses
