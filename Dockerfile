# In this image, we avoid using alpine due to performance issues
# with musl. We use debian slim so we can use glibc for best performance.

# Start by building the nitriding proxy daemon.
FROM public.ecr.aws/docker/library/golang:1.22.1-bookworm@sha256:d996c645c9934e770e64f05fc2bc103755197b43fd999b3aa5419142e1ee6d78 as go-builder

RUN CGO_ENABLED=0 go install -trimpath -ldflags="-s -w" -buildvcs=false github.com/brave/nitriding-daemon@v1.4.2

# Build the web server application itself.
FROM public.ecr.aws/docker/library/rust:1.76.0-bookworm@sha256:d36f9d8a9a4c76da74c8d983d0d4cb146dd2d19bb9bd60b704cdcf70ef868d3a as rust-builder

WORKDIR /src/
COPY Cargo.toml Cargo.lock ./
COPY src src/
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

# Set up the run-time environment
FROM public.ecr.aws/docker/library/debian:12.5-slim@sha256:ccb33c3ac5b02588fc1d9e4fc09b952e433d0c54d8618d0ee1afadf1f3cf2455

RUN apt update && apt install -y ca-certificates

COPY start.sh /usr/local/bin
RUN chown root:root /usr/local/bin/start.sh
RUN chmod 755 /usr/local/bin/start.sh

COPY --from=go-builder /go/bin/nitriding-daemon /usr/local/bin/nitriding
COPY --from=rust-builder /src/target/release/star-randsrv /usr/local/bin/

EXPOSE 443
# Switch to the UID that's typically reserved for the user "nobody".
USER 65534

CMD ["/usr/local/bin/start.sh"]
