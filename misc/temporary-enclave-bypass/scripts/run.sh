#!/bin/bash

cpu_count=${2:-2}
memory=${3:-512}
cid="4"

set -eux

nitro-cli run-enclave \
    --enclave-cid "${cid}" \
    --cpu-count ${cpu_count} \
    --memory ${memory} \
    --eif-path nitro-image.eif > /tmp/output.json
cat /tmp/output.json

# background the proxy startup
/usr/local/bin/start-proxies.sh "${cid}" &

# run star-randsrv
echo "Starting star-randsrv."
star-randsrv \
  --epoch-seconds 604800 \
  --epoch-base-time 2023-05-01T00:00:00Z \
  --increase-nofile-limit \
  --listen "127.0.0.1:8081" \
  --prometheus-listen "0.0.0.0:9090"

