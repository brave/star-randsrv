# Start by building the sta-rs library in a builder container.
FROM rust:1.60 as rust-builder

WORKDIR /src/
COPY sta-rs ./sta-rs
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cd sta-rs/ppoprf/ffi && cargo build --locked --release

# Take the compiled sta-rs library (specifically, the object and header file),
# and use it to build star-randsrv; again, in a builder container.
FROM golang:1.18 as go-builder

WORKDIR /src/
RUN mkdir -p ./sta-rs/target/release ./sta-rs/ppoprf/ffi/include
COPY --from=rust-builder /src/sta-rs/ppoprf/ffi/include/ppoprf.h ./sta-rs/ppoprf/ffi/include
COPY --from=rust-builder /src/sta-rs/target/release/libffi.a ./sta-rs/target/release

COPY *.go go.mod go.sum ./
RUN go mod download
RUN go build -trimpath -o star-randsrv ./

# Copy from the builder to keep the final image reproducible and small.  If we
# don't do this, we end up with non-deterministic build artifacts.
FROM scratch
COPY --from=go-builder /src/star-randsrv /
EXPOSE 8080
CMD ["/star-randsrv"]
