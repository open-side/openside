# Build from the repository root:
#   docker build -f crates/openside-relay/Dockerfile -t openside-relay .
# Optional: build the web app first (pnpm -C web build) so the relay can serve it to phones.
FROM rust:1-slim-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release --locked -p openside-relay \
 && mkdir -p /out/web \
 && cp target/release/openside-relay /out/ \
 && if [ -d web/dist ]; then cp -r web/dist/. /out/web/; fi

FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=build /out/openside-relay /usr/local/bin/openside-relay
COPY --from=build /out/web /srv/openside-web
ENV OPENSIDE_RELAY_LISTEN=0.0.0.0:8080 \
    OPENSIDE_RELAY_WEB_DIR=/srv/openside-web
EXPOSE 8080
USER nonroot
ENTRYPOINT ["/usr/local/bin/openside-relay"]
