#!/bin/sh

nitriding \
  -fqdn "star-randsrv.bsg.brave.software" \
  -fqdn-leader "leader-internal-sync-service.star-randsrv-dev.svc.cluster.local:9443" \
  -appurl "https://github.com/brave/star-randsrv" \
  -appwebsrv "http://127.0.0.1:8080" \
  -vsock-ext \
  -disable-keep-alives \
  -host-ip-provider-port 6161 \
  -ext-priv-port 9443 \
  -intport 8081 &
echo "[sh] Started nitriding as reverse proxy."

sleep 1

star-randsrv \
  --instance-name typical \
  --epoch-duration "1w" \
  --instance-name express \
  --epoch-duration "1d" \
  --instance-name slow \
  --epoch-duration "1mon" \
  --epoch-base-time 2023-05-01T00:00:00Z \
  --increase-nofile-limit \
  --enclave-key-sync \
  --nitriding-internal-port 8081

echo "[sh] Started star-randsrv."
