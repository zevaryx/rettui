# rettui web UI. Build from a checkout with its submodules:
#   git submodule update --init
#   docker compose up -d --build
# The login link is in the logs: docker compose logs rettui

FROM rust:1-slim-bookworm AS build
WORKDIR /src
# The Reticulum, LXMF and NomadNet libraries are git submodules in deps/.
COPY Cargo.toml Cargo.lock ./
COPY deps ./deps
COPY src ./src
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    test -f deps/rsReticulum/Cargo.toml \
        || { echo "deps/ is empty: run 'git submodule update --init' first" >&2; exit 1; } \
    && cargo build --release --locked \
    && cp target/release/rettui /rettui

FROM debian:bookworm-slim
# setpriv (util-linux, already in the image) drops to PUID/PGID at start.
COPY --from=build /rettui /usr/local/bin/rettui
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod 0755 /usr/local/bin/entrypoint.sh /usr/local/bin/rettui \
    && command -v setpriv >/dev/null

# Everything rettui keeps (identity, settings, messages, Reticulum config)
# lives in /data; HOME points there too so nothing is written elsewhere.
# Set PUID and PGID (or GUID) for the owner of /data, and PORT for the port;
# they default to 1000, 1000 and 8740 in the entrypoint.
ENV HOME=/data
VOLUME /data
EXPOSE 8740
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
