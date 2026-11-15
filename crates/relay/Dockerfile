FROM --platform=$BUILDPLATFORM ghcr.io/rust-cross/cargo-zigbuild:0.24.1 AS build
ARG TARGETARCH
WORKDIR /src
COPY . .
RUN case "$TARGETARCH" in \
      amd64) target=x86_64-unknown-linux-musl ;; \
      arm64) target=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported arch $TARGETARCH" >&2; exit 1 ;; \
    esac \
  && rustup target add "$target" \
  && cargo zigbuild --release --locked -p ludolph-relay --target "$target" \
  && cp "target/$target/release/ludolph-relay" /ludolph-relay

FROM scratch
COPY --from=build /ludolph-relay /ludolph-relay
ENV BIND_ADDR=0.0.0.0:8080 DATA_DIR=/data
EXPOSE 8080
USER 10001:10001
ENTRYPOINT ["/ludolph-relay"]
