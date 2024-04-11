# Build the web server application itself.
# Use the -alpine variant so it will run in a alpine-based container.
FROM public.ecr.aws/docker/library/rust:1.77.1-alpine@sha256:d4c2b0a1544462f40b6179aedff4f5485a019a213907c8590ed77d1b6145a29c as rust-builder
# Base image may not support C linkage.
RUN apk add musl-dev

WORKDIR /src/
COPY Cargo.toml Cargo.lock ./
COPY src src
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

RUN cargo install vsock-relay

FROM amazonlinux:2.0.20240329.0@sha256:85825c659f9d0d51218492aab1f71a1d5adae074e95019b5518c071249a9ec95

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
