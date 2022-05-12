.PHONY: all test lint eif star-randsrv clean

binary = star-randsrv
image = $(binary):latest
godeps = *.go go.mod go.sum
stardeps = include/ppoprf.h target/release/libstar_ppoprf_ffi.a

all: test lint $(binary)

test: $(godeps) $(stardeps)
	go test -cover ./...
	cargo test

lint:
	golangci-lint run ./...

image:
	$(eval IMAGE=$(shell ko publish --local . 2>/dev/null))
	@echo "Built image URI: $(IMAGE)."
	$(eval DIGEST=$(shell echo $(IMAGE) | cut -d ':' -f 2))
	@echo "SHA-256 digest: $(DIGEST)"

eif: image
	nitro-cli build-enclave --docker-uri $(IMAGE) --output-file ko.eif
	$(eval ENCLAVE_ID=$(shell nitro-cli describe-enclaves | jq -r '.[0].EnclaveID'))
	@if [ "$(ENCLAVE_ID)" != "null" ]; then nitro-cli terminate-enclave --enclave-id $(ENCLAVE_ID); fi
	@echo "Starting enclave."
	nitro-cli run-enclave --cpu-count 2 --memory 2500 --enclave-cid 4 --eif-path ko.eif --debug-mode
	@echo "Showing enclave logs."
	nitro-cli console --enclave-id $$(nitro-cli describe-enclaves | jq -r '.[0].EnclaveID')

docker:
	@docker run \
		-v $(PWD):/workspace \
		--network=host \
		gcr.io/kaniko-project/executor:v1.7.0 \
		--reproducible \
		--dockerfile /workspace/Dockerfile \
		--no-push \
		--tarPath /workspace/$(binary)-repro.tar \
		--destination $(image) \
		--context dir:///workspace/ && cat $(binary)-repro.tar | docker load >/dev/null
	@rm -f $(binary)-repro.tar
	@echo $(image)

$(binary): $(godeps) $(stardeps)
	go build -o $(binary)

$(stardeps): Cargo.toml build.rs src/lib.rs cbindgen.toml
	cargo build --release

clean:
	@rm -f $(binary)
	@cargo clean
