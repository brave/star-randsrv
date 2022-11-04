# Start by building the sta-rs library in a builder container.
FROM rust:1.60 as rust-builder

WORKDIR /src/
COPY . .
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

# Take the compiled sta-rs library (specifically, the object and header file),
# and use it to build star-randsrv; again, in a builder container.
FROM golang:1.19 as go-builder

WORKDIR /src/
RUN mkdir -p ./target/release ./include
COPY --from=rust-builder /src/include/ppoprf.h ./include
COPY --from=rust-builder /src/target/release/libstar_ppoprf_ffi.a ./target/release

COPY *.go go.mod go.sum ./
RUN go mod download
RUN go build -trimpath -o star-randsrv ./

# Copy from the builder to keep the final image reproducible and small.  If we
# don't do this, we end up with non-deterministic build artifacts.
FROM scratch
COPY --from=go-builder /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
COPY --from=go-builder /src/star-randsrv /
EXPOSE 8443
# Switch to the UID that's typically reserved for the user "nobody".
USER 65534
CMD ["/star-randsrv"]
