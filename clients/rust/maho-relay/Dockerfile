FROM rust:1-bookworm AS build
WORKDIR /ws
RUN mkdir -p clients/rust && printf '[workspace]\nresolver = "2"\nmembers = ["maho-relay", "maho-proto"]\n\n[workspace.package]\nedition = "2021"\nlicense = "MIT"\nrust-version = "1.80"\n\n[workspace.dependencies]\ntokio = { version = "1", features = ["full"] }\ntracing = "0.1"\ntracing-subscriber = { version = "0.3", features = ["env-filter"] }\nserde = { version = "1", features = ["derive"] }\nserde_json = "1"\nthiserror = "1"\nanyhow = "1"\nsha2 = "0.10"\n\n[profile.release]\nstrip = true\n' > clients/rust/Cargo.toml
COPY Cargo.toml clients/rust/maho-relay/Cargo.toml
COPY src clients/rust/maho-relay/src
COPY proto-vendor/Cargo.toml clients/rust/maho-proto/Cargo.toml
COPY proto-vendor/src clients/rust/maho-proto/src
COPY Cargo.lock-relay clients/rust/Cargo.lock
WORKDIR /ws/clients/rust
RUN cargo build --release -p maho-relay

FROM debian:bookworm-slim
COPY --from=build /ws/clients/rust/target/release/maho-relay /usr/local/bin/maho-relay
ENV RELAY_BIND=0.0.0.0:8080
EXPOSE 8080
CMD ["maho-relay"]
