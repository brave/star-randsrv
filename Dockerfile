# In this image, we avoid using alpine due to performance issues
# with musl. We use debian slim so we can use glibc for best performance.

# Build the web server application itself.
FROM public.ecr.aws/docker/library/rust:1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS rust-builder

WORKDIR /src/
COPY Cargo.toml Cargo.lock ./
COPY src src/
# The '--locked' argument is important for reproducibility because it ensures
# that we use specific dependencies.
RUN cargo build --locked --release

# Set up the run-time environment
FROM public.ecr.aws/docker/library/debian:12-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc

COPY --from=rust-builder /src/target/release/star-randsrv /usr/local/bin/

EXPOSE 8080
# Switch to the UID that's typically reserved for the user "nobody".
USER 65534

CMD ["/usr/local/bin/star-randsrv"]
