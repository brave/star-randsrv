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

Output
------
