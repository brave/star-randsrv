#!/bin/sh

nitriding \
	-fqdn "star-randsrv.bsg.brave.com" \
	-appurl "https://github.com/brave/star-randsrv" \
	-appwebsrv "http://127.0.0.1:8080" \
	-prometheus-port 9090 \
	-vsock-ext \
	-disable-keep-alives \
	-prometheus-namespace "nitriding" \
	-extport 443 \
	-intport 8081 &
echo "[sh] Started nitriding as reverse proxy."

sleep 1

star-randsrv \
  --epoch-seconds 604800 \
  --epoch-base-time 2023-05-01T00:00:00Z \
  --increase-nofile-limit
echo "[sh] Started star-randsrv."
