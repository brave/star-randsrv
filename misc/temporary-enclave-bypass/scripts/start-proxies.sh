#!/bin/bash

set -eux

CID="${1}"
PARENT_CID="3" # the CID of the EC2 instance

echo "cid is ${CID}"
# it's now time to set up proxy tools

# run vsock relay to proxy enclave attestation requests
/usr/local/bin/vsock-relay -s "127.0.0.1:8443" -l "4:443" -c 1000 &

# run nginx to proxy attestation & randsrv requests
nginx
