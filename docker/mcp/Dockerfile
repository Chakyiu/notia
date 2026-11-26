# Build context is the repository root. See docker/mcp/compose.yaml.
# 1.88 or newer: the sources use let chains, which 1.85 rejects.
#
# Compile on the machine's own architecture. Running the amd64 rustc under
# QEMU segfaults (`rustc -vV`, signal 11), so a foreign image is produced
# with a cross linker instead of by emulating the compiler.
FROM --platform=$BUILDPLATFORM rust:1.100-bookworm AS build

ARG TARGETARCH

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        gcc-aarch64-linux-gnu \
        gcc-x86-64-linux-gnu \
        libc6-dev-arm64-cross \
        libc6-dev-amd64-cross \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add aarch64-unknown-linux-gnu x86_64-unknown-linux-gnu

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CARGO_TARGET_X86_64_UNKNOWN_LINUX_GNU_LINKER=x86_64-linux-gnu-gcc

WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# The workspace patches gpui-component to this path. Cargo checks the patch
# even when building notia-mcp, which does not link that crate.
COPY third_party/gpui-component ./third_party/gpui-component
RUN set -eu; \
    case "$TARGETARCH" in \
        amd64) target=x86_64-unknown-linux-gnu ;; \
        arm64) target=aarch64-unknown-linux-gnu ;; \
        *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    cargo build --locked --release --target "$target" \
        -p notia-mcp --bin notia-mcp; \
    cp "target/${target}/release/notia-mcp" /notia-mcp

FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=build /notia-mcp /usr/local/bin/notia-mcp

# The vault is a bind mount of the user's notes. This image does not chown
# that directory. compose.yaml can set a user that is allowed to write it.
WORKDIR /vault
EXPOSE 8788

ENTRYPOINT ["/usr/local/bin/notia-mcp"]
CMD ["--vault", "/vault", "--listen", "0.0.0.0:8788"]
