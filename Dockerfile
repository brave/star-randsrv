# Start by building the nitriding proxy daemon.
FROM public.ecr.aws/docker/library/golang:1.20 as go-builder

WORKDIR /src/
COPY . .
RUN make -C nitriding/cmd nitriding

# Build the web server application itself.
# Use the -alpine variant so it will run in a alpine-based container.
FROM public.ecr.aws/docker/library/rust:1.68.2-alpine as rust-builder
# Base image may not support C linkage.
RUN apk add musl-dev

WORKDIR /src/
COPY . .
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

# Copy from the builder imagse to keep the final image reproducible and small,
# and to improve reproducibilty of the build.
FROM public.ecr.aws/docker/library/alpine:3.17.3
COPY --from=go-builder /src/nitriding/cmd/nitriding /usr/local/bin/
COPY --from=rust-builder /src/target/release/star-randsrv /usr/local/bin/

# Set up the run-time environment
COPY start.sh /usr/local/bin/

EXPOSE 443
# Switch to the UID that's typically reserved for the user "nobody".
USER 65534

CMD ["/usr/local/bin/start.sh"]
