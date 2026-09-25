# syntax=docker/dockerfile:1
# Builds solidate-server, solidate (CLI) and solidate-mcp into one runtime image.

FROM rust:1.98.1-slim-trixie AS build
WORKDIR /src
COPY . .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked --bins \
    && mkdir /out \
    && cp target/release/solidate target/release/solidate-server target/release/solidate-mcp /out/

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --system --uid 10001 solidate
COPY --from=build /out/ /usr/local/bin/
USER solidate
ENV HOST=0.0.0.0 \
    PORT=3000
EXPOSE 3000
HEALTHCHECK --interval=10s --timeout=3s --retries=3 CMD curl -fsS http://127.0.0.1:3000/healthz || exit 1
CMD ["solidate-server"]
