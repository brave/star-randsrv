# In this image, we avoid using alpine due to performance issues
# with musl. We use debian slim so we can use glibc for best performance.

# Start by building the nitriding proxy daemon.
FROM public.ecr.aws/docker/library/golang:1.23.0-bookworm@sha256:31dc846dd1bcca84d2fa231bcd16c09ff271bcc1a5ae2c48ff10f13b039688f3 as go-builder

RUN CGO_ENABLED=0 go install -trimpath -ldflags="-s -w" -buildvcs=false github.com/brave/nitriding-daemon@v1.4.2

# Build the web server application itself.
FROM public.ecr.aws/docker/library/rust:1.80.1-bookworm@sha256:29fe4376919e25b7587a1063d7b521d9db735fc137d3cf30ae41eb326d209471 as rust-builder

WORKDIR /src/
COPY Cargo.toml Cargo.lock ./
COPY src src/
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

# Set up the run-time environment
FROM public.ecr.aws/docker/library/debian:12.10-slim@sha256:b1211f6d19afd012477bd34fdcabb6b663d680e0f4b0537da6e6b0fd057a3ec3

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
