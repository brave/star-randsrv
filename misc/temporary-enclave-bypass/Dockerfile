# Build the web server application itself.
# Use the -alpine variant so it will run in a alpine-based container.
FROM public.ecr.aws/docker/library/rust:1.78.0-alpine@sha256:214477ec837f9bedd80be4b087fec09e3f270831979412840f3f6c38e5a0d9c1 as rust-builder
# Base image may not support C linkage.
RUN apk add musl-dev

WORKDIR /src/
COPY Cargo.toml Cargo.lock ./
COPY src src
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

RUN cargo install vsock-relay

FROM amazonlinux:2.0.20240529.0@sha256:cb70fdaf99571444d0059810e27b0c6a192a5e940a77184fa95df00a1e88dac3

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
