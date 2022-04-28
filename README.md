STAR Randomness Server
======================

This service implements a service that serves as a front end to the randomness
server that's proposed in the paper [STAR: Distributed Secret Sharing for
Private Threshold Aggregation Reporting](https://arxiv.org/abs/2109.10074).  The
actual randomness server implementation (written in Rust) can be found in the
[sta-rs](https://github.com/brave-experiments/sta-rs) repository; this
repository merely implements a Go wrapper that can be run inside an AWS Nitro
Enclave.

Installation
------------

Input
-----

The randomness server exposes an HTTP POST request handler at `/randomness`.
The handler expects a JSON-formatted request body.  Below is an example of a
valid request body.

```
{
  "points": [
    "90aaa9713616607aed7a0eb685511c5862e4c3a2fecf21a748bce5b33077492e",
    "a4e0b94d2cb2d93ac397caaf0bb1798224fbec29da4ebce6cd134d3970d2de0e",
    "8297f13c55137c9bca743eb1e46ef8543f41c5d6f779fb0760937477bbeed171"
  ]
}
```

The JSON array `points` contains a list of one or more hex-encoded
[Ristretto](https://github.com/bwesterb/go-ristretto) points.

Output
------

The randomness server's response contains the same JSON structure but its
points are punctured based on the client-provided input and the server's secret
key.  Refer to the [STAR paper](https://arxiv.org/abs/2109.10074) for details.
Below is an example of the server's response:

```
{
  "points": [
    "36c597dd76699a38adfd7f05a7ce48f756ea15a1f256895c96bbe7b597a94362",
    "24116ce3b99a7c0aa438aa3de952af76f493033f1c4ce0336f9f0715d55a4c41",
    "9617887e50ec4d5f1a525fe2cd10214c2e7949eaa2db0e6366435c902988ca1a"
  ]
}
```

Note that the array's ordering matters.  The point at index n of the server's
response corresponds to the point at index n of the client's request.
