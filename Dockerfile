# In this image, we avoid using alpine due to performance issues
# with musl. We use debian slim so we can use glibc for best performance.

# Start by building the nitriding proxy daemon.
FROM public.ecr.aws/docker/library/golang:1.22.3-bookworm@sha256:5c56bd47228dd572d8a82971cf1f946cd8bb1862a8ec6dc9f3d387cc94136976 as go-builder

RUN CGO_ENABLED=0 go install -trimpath -ldflags="-s -w" -buildvcs=false github.com/brave/nitriding-daemon@v1.4.2

# Build the web server application itself.
FROM public.ecr.aws/docker/library/rust:1.77.2-bookworm@sha256:83101f6985c93e1e6501b3375de188ee3d2cbb89968bcc91611591f9f447bd42 as rust-builder

WORKDIR /src/
COPY Cargo.toml Cargo.lock ./
COPY src src/
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

# Set up the run-time environment
FROM public.ecr.aws/docker/library/debian:12.5-slim@sha256:804194b909ef23fb995d9412c9378fb3505fe2427b70f3cc425339e48a828fca

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
