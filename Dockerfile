# rettui web UI. Build from a checkout with its submodules:
#   git submodule update --init
#   docker compose up -d --build
# The login link is in the logs: docker compose logs rettui
#
# Published images (.github/workflows/docker.yml, for releases and
# branches) use `--target prebuilt` with binaries the workflow has already
# built, at dist/rettui-<arch> (amd64, arm64), so nothing is compiled under
# emulation.

FROM rust:1-slim-bookworm AS build
WORKDIR /src
# The Reticulum, LXMF and NomadNet libraries are git submodules in deps/;
# Codec2 is vendored in vendor/.
COPY Cargo.toml Cargo.lock ./
COPY deps ./deps
COPY vendor ./vendor
COPY src ./src
# The desktop app is in the same workspace (only its manifest is read).
COPY desktop ./desktop
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    test -f deps/rsReticulum/Cargo.toml \
        || { echo "deps/ is empty: run 'git submodule update --init' first" >&2; exit 1; } \
    && cargo build --release --locked \
    && cp target/release/rettui /rettui

FROM debian:bookworm-slim AS runtime
# setpriv (util-linux, already in the image) drops to PUID/PGID at start.
COPY --chmod=0755 docker/entrypoint.sh /usr/local/bin/entrypoint.sh

# Everything rettui keeps (identity, settings, messages, Reticulum config)
# lives in /data; HOME points there too so nothing is written elsewhere.
# Set PUID and PGID (or GUID) for the owner of /data, and PORT for the port;
# they default to 1000, 1000 and 8740 in the entrypoint.
ENV HOME=/data
# A container's rettui is the image's: updated by pulling a new image, not
# by replacing itself (see src/update/install.rs).
ENV RETTUI_CONTAINER=1
VOLUME /data
# 8740: the web UI. 4242: Reticulum's usual port, for running a transport
# node others connect to (a TCPServerInterface on 0.0.0.0:4242 in
# /data/reticulum/config, with enable_transport).
EXPOSE 8740 4242
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]

FROM runtime AS prebuilt
ARG TARGETARCH
COPY --chmod=0755 dist/rettui-${TARGETARCH} /usr/local/bin/rettui

# The default: build from source.
FROM runtime
COPY --chmod=0755 --from=build /rettui /usr/local/bin/rettui
