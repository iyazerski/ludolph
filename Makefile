test:
	cargo test --workspace --locked

# Regenerate THIRD_PARTY_LICENSES.md after changing dependencies; CI checks it's current.
licenses:
	cargo about generate --workspace --fail about.hbs > THIRD_PARTY_LICENSES.md

.PHONY: test licenses
