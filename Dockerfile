# Start by building the nitriding proxy daemon.
FROM public.ecr.aws/docker/library/golang:1.20.6-alpine as go-builder

RUN CGO_ENABLED=0 go install -trimpath -ldflags="-s -w" -buildvcs=false github.com/brave/nitriding-daemon@v1.2.1

# Build the web server application itself.
# Use the -alpine variant so it will run in a alpine-based container.
FROM public.ecr.aws/docker/library/rust:1.71.0-alpine as rust-builder
# Base image may not support C linkage.
RUN apk add musl-dev

WORKDIR /src/
COPY Cargo.toml Cargo.lock .
COPY src src
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

FROM public.ecr.aws/docker/library/alpine:3.18.2 as file-builder

# Set up the run-time environment
COPY start.sh /
RUN chown root:root /start.sh
RUN chmod 755 /start.sh

# Copy from the builder imagse to keep the final image reproducible and small,
# and to improve reproducibilty of the build.
FROM public.ecr.aws/docker/library/alpine:3.18.2
COPY --from=go-builder /go/bin/nitriding-daemon /usr/local/bin/nitriding
COPY --from=rust-builder /src/target/release/star-randsrv /usr/local/bin/
COPY --from=file-builder /start.sh /usr/local/bin/

EXPOSE 443
# Switch to the UID that's typically reserved for the user "nobody".
USER 65534

CMD ["/usr/local/bin/start.sh"]
