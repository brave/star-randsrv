STAR Randomness Server
======================

This repository implements the randomness server that's proposed
in the paper [STAR: Distributed Secret Sharing
for Private Threshold Aggregation Reporting](https://arxiv.org/abs/2109.10074).
The actual oblivious pseudorandom function implementation can be found
in the [sta-rs](https://github.com/brave/sta-rs) repository.
This repository implements webservice wrapper to make evaluation available
over the network.

It also includes a reproducible container build that can be run inside an
AWS Nitro Enclave, providing remote attestation of the implementation and
additional security for the private key.

Installation
------------

To test, lint, and build the randomness server, simply run:

```
make
```

To execute just the randomness webapp with logging, run:

```
RUST_LOG=tower_http=trace,star_randsrv=debug cargo run
```

To build a reproducible container image of the randomness server, run:

```
make image
```

Input
-----

The randomness server exposes an HTTP POST request handler at `/randomness`.
The handler expects a JSON-formatted request body.  Below is an example of a
valid request body.

```
{
  "points": [
    "uqUmPbpGjpqaQcVnbn39PZGtL4DjfY+h9R+XqlKLuVc=",
    "CCBnmLsPR8hFzuxhRz0a05TAh+p0jFhebMCDgOcfdWk=",
    "bNQSygww5ykQpfsDMJXTiaX/MmpWW4qnfmuRpdR/1yY="
  ]
}
```

The JSON array `points` contains a list of one or more Base64-encoded
[Ristretto](https://github.com/bwesterb/go-ristretto) points.

Output
------

The randomness server's response contains a similar JSON structure but its
points are punctured based on the client-provided input and the server's secret
key.  Refer to the [STAR paper](https://arxiv.org/abs/2109.10074) for details.
Below is an example of the server's response:

```
{
  "epoch": 0,
  "points": [
    "qC3vaUizBSrNZCCkzD3jBhHqMEWZIuNj5IdNk57GGHY=",
    "rh7Tcr1LqwVQVtCEEIZqwUCPDvBOMM5bJPA8EfShnzI=",
    "Bq8LJ0KpfwQHgh1tkr8OP+ogmxPQz7lWHfAPuyVxXU0="
  ]
}
```

Note that the array's ordering matters.  The point at index *n* of the server's
response corresponds to the point at index *n* of the client's request.

Reproducible builds
----
Executing `make eif` will render a reproducible Nitro Enclave image. The ID of the image
can be compared the with image ID in the attestation document served at https://star-randsrv.bsg.brave.com/enclave/attestation
for auditing purposes (See [nitriding-daemon](https://github.com/brave/nitriding-daemon) for details).
Currently, there is an outstanding kernel leak bug within the stock kernel packaged
with the aws-nitro-enclaves-cli. A [custom-built kernel](https://github.com/brave-experiments/nitro-enclave-kernel) must be
used when building the image.

