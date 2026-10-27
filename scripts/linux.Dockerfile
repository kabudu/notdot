FROM rust:1.97-slim-bookworm
RUN apt-get update && apt-get install -y --no-install-recommends build-essential pkg-config libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf libssl-dev libdbus-1-dev libasound2-dev file curl ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /work
