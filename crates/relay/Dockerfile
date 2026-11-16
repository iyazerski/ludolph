FROM --platform=$BUILDPLATFORM lukemathwalker/cargo-chef:0.1.78-rust-1 AS chef

FROM --platform=$BUILDPLATFORM ghcr.io/rust-cross/cargo-zigbuild:0.24.1 AS base
COPY --from=chef /usr/local/cargo/bin/cargo-chef /usr/local/cargo/bin/
WORKDIR /src

FROM base AS plan
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Dependencies build in their own layer, keyed on the manifests and Cargo.lock,
# so a source change reuses them from the build cache.
FROM base AS build
ARG TARGETARCH
RUN case "$TARGETARCH" in \
      amd64) echo x86_64-unknown-linux-musl ;; \
      arm64) echo aarch64-unknown-linux-musl ;; \
      *) echo "unsupported arch $TARGETARCH" >&2; exit 1 ;; \
    esac > /target \
  && rustup target add "$(cat /target)"
COPY --from=plan /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --zigbuild -p ludolph-relay --target "$(cat /target)" --recipe-path recipe.json
COPY . .
RUN target="$(cat /target)" \
  && cargo zigbuild --release --locked -p ludolph-relay --target "$target" \
  && cp "target/$target/release/ludolph-relay" /ludolph-relay

FROM scratch
COPY --from=build /ludolph-relay /ludolph-relay
ENV BIND_ADDR=0.0.0.0:8080 DATA_DIR=/data
EXPOSE 8080
USER 10001:10001
ENTRYPOINT ["/ludolph-relay"]
