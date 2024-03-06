# Build the web server application itself.
# Use the -alpine variant so it will run in a alpine-based container.
FROM public.ecr.aws/docker/library/rust:1.76.0-alpine@sha256:def35884ff7e1e4b0eae050cbb03ca0900d70d1acbcba1d0e428147ab6786de2 as rust-builder
# Base image may not support C linkage.
RUN apk add musl-dev

WORKDIR /src/
COPY Cargo.toml Cargo.lock ./
COPY src src
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

RUN cargo install vsock-relay

FROM amazonlinux:2.0.20240223.0@sha256:b8607887eebccfafa27494fc22841db4f1d707e506b0df293077dc1af2d027d0

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
