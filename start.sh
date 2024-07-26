#!/bin/sh

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
  --nitriding-internal-port 8081 &
star_randsrv_pid=$!
echo "[sh] Started star-randsrv."

sleep 5

nitriding \
  -fqdn "star-randsrv.bsg.brave.software" \
  -fqdn-leader "leader-internal-service.star-randsrv-dev.svc.cluster.local:9443" \
  -appurl "https://github.com/brave/star-randsrv" \
  -appwebsrv "http://127.0.0.1:8080" \
  -vsock-ext \
  -disable-keep-alives \
  -prometheus-port 9090 \
  -host-ip-provider-port 6161 \
  -ext-priv-port 9443 \
  -intport 8081 &
nitriding_pid=$!
echo "[sh] Started nitriding as reverse proxy."

while true; do
  ([ -d "/proc/$nitriding_pid" ] && [ -d "/proc/$star_randsrv_pid" ]) || exit 1
  sleep 5
done
