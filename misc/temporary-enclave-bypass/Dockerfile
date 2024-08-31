# Build the web server application itself.
# Use the -alpine variant so it will run in a alpine-based container.
FROM public.ecr.aws/docker/library/rust:1.80.1-alpine@sha256:1f5aff501e02c1384ec61bb47f89e3eebf60e287e6ed5d1c598077afc82e83d5 as rust-builder
# Base image may not support C linkage.
RUN apk add musl-dev

WORKDIR /src/
COPY Cargo.toml Cargo.lock ./
COPY src src
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

RUN cargo install vsock-relay

FROM amazonlinux:2.0.20240816.0@sha256:d02b4bac5d085fa4e8f6e397e21315683ed11ae907b421177859e317b59e2f7c

RUN echo "timeout=60.0" >> /etc/yum.conf
RUN amazon-linux-extras install aws-nitro-enclaves-cli nginx1 -y && \
    yum install aws-nitro-enclaves-cli-devel wget curl strace awscli -y && \
    yum clean all && \
    rm -rf /var/cache/yum && \
    mkdir -p /enclave

COPY --from=rust-builder /src/target/release/star-randsrv /usr/local/bin/
COPY --from=rust-builder /usr/local/cargo/bin/vsock-relay /usr/local/bin/
COPY ./misc/temporary-enclave-bypass/scripts/*.sh /usr/local/bin/
COPY ./misc/temporary-enclave-bypass/nginx.conf /etc/nginx/

EXPOSE 8080
