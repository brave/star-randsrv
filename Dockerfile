FROM golang:1.18 as builder

WORKDIR /src/
COPY *.go go.mod go.sum ./
COPY sta-rs ./sta-rs
RUN go mod download
RUN go build -trimpath -o star-randsrv ./

# Copy from the builder to keep the final image reproducible and small.  If we
# don't do this, we end up with non-deterministic build artifacts.
FROM scratch
COPY --from=builder /src/star-randsrv /
EXPOSE 8080
CMD ["/star-randsrv"]
