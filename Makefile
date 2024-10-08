prog := star-randsrv
version := $(shell git describe --tag --dirty)
image_tag := $(prog):$(version)
image_tar := $(prog)-$(version).tar.gz
image_eif := $(image_tar:%.tar.gz=%.eif)

RUST_DEPS := $(wildcard Cargo.* src/*.rs)

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

eif: $(image_eif)

$(image_eif): $(image_tar)
	gunzip -c $(image_tar) | docker load
	nitro-cli build-enclave --docker-uri $(image_tag) --output-file $@

image: $(image_tar)

$(image_tar): default.nix $(RUST_DEPS)
	nix-build -v --arg tag \"$(version)\"
	rm -f $(image_tar)
	cp -L ./result $(image_tar)

run: $(image_eif)
	$(eval ENCLAVE_ID=$(shell nitro-cli describe-enclaves | jq -r '.[0].EnclaveID'))
	@if [ "$(ENCLAVE_ID)" != "null" ]; then nitro-cli terminate-enclave --enclave-id $(ENCLAVE_ID); fi
	@echo "Starting enclave."
	nitro-cli run-enclave --cpu-count 4 --memory 2048 --eif-path $(image_eif) --debug-mode
	@echo "Showing enclave logs."
	nitro-cli console --enclave-id $$(nitro-cli describe-enclaves | jq -r '.[0].EnclaveID')
