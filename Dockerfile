# Builds the Rust orchestrator together with the C++ simulation kernel it links.

FROM rust:1-slim-bookworm AS build
RUN apt-get update \
 && apt-get install -y --no-install-recommends g++ \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /src
COPY core ./core
COPY orchestrator ./orchestrator
WORKDIR /src/orchestrator
RUN cargo build --release --locked

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends libstdc++6 \
 && rm -rf /var/lib/apt/lists/* \
 && useradd --uid 10001 --no-create-home --shell /usr/sbin/nologin fleet
COPY --from=build /src/orchestrator/target/release/fleetctl /usr/local/bin/fleetctl
USER 10001
ENTRYPOINT ["fleetctl"]
CMD ["info"]
