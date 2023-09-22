FROM rust:slim-buster as builder

WORKDIR /gatling

COPY . .
RUN cargo build --release

FROM debian:buster-slim
LABEL description="Gomu Gomu no Gatling" \
  authors="Oak <oak@lambdaclass.com>" \
  source="https://github.com/keep-starknet-strange/gomu-gomu-no-gatling"

COPY --from=builder /gatling/target/release/gatling /gatling-bin

ENTRYPOINT ["/gatling-bin"]
