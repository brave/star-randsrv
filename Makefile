prog := star-randsrv
version := $(shell git describe --tag --dirty)
image_tag := $(prog):$(version)
image_tar := $(prog)-$(version)-kaniko.tar
image_eif := $(image_tar:%.tar=%.eif)

RUST_DEPS := $(wildcard Cargo.* src/*.rs)
NITRIDING_DEPS := $(wildcard nitriding/*.go) $(wildcard nitriding/cmd/main.go)

# RUST_DEPS is approximate; always invoke cargo to update $(prog).
.PHONY: all test lint clean eif image target/release/$(prog)

all: test lint target/release/$(prog)

test:
	cargo test

lint:
	cargo clippy
	cargo audit

target/release/$(prog): Cargo.toml src/*.rs
	cargo build --release

clean:
	cargo clean
	$(RM) $(image_tar)
	$(RM) $(image_eif)

# Check out the nitriding submodule if it hasn't been already
nitriding/cmd/Makefile:
	git submodule update --init

eif: $(image_eif)

$(image_eif): $(image_tar)
	docker load -i $<
	nitro-cli build-enclave --docker-uri $(image_tag) --output-file $@

image: $(image_tar)

$(image_tar): Dockerfile $(RUST_DEPS) $(NITRIDING_DEPS) nitriding/cmd/Makefile
	docker run -v $$PWD:/workspace gcr.io/kaniko-project/executor:v1.9.2 \
		--context dir:///workspace/ \
		--reproducible \
		--no-push \
		--tarPath $(image_tar) \
		--destination $(image_tag) \
		--custom-platform linux/amd64

run: $(image_eif)
	$(eval ENCLAVE_ID=$(shell nitro-cli describe-enclaves | jq -r '.[0].EnclaveID'))
	@if [ "$(ENCLAVE_ID)" != "null" ]; then nitro-cli terminate-enclave --enclave-id $(ENCLAVE_ID); fi
	@echo "Starting enclave."
	nitro-cli run-enclave --cpu-count 2 --memory 512 --eif-path $(image_eif) --debug-mode
	@echo "Showing enclave logs."
	nitro-cli console --enclave-id $$(nitro-cli describe-enclaves | jq -r '.[0].EnclaveID')
