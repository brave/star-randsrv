package main

/*
#cgo LDFLAGS: -L sta-rs/target/release -lffi -lpthread -ldl -static
#cgo CFLAGS: -I sta-rs/ppoprf/ffi/include -O3
#include "sta-rs/ppoprf/ffi/include/ppoprf.h"
*/
import "C"

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io/ioutil"
	"log"
	"net/http"
	"os"
	"runtime"
	"sync"
	"time"
	"unsafe"

	// This module must be imported first because of its side effects of
	// seeding our system entropy pool.
	_ "github.com/brave-experiments/nitro-enclave-utils/randseed"

	nitro "github.com/brave-experiments/nitro-enclave-utils"
	"github.com/bwesterb/go-ristretto"
)

var (
	elog             = log.New(os.Stderr, "star-randsrv: ", log.Ldate|log.Ltime|log.LUTC|log.Lshortfile)
	errNoReqBody     = "no request body"
	errBadJSON       = "failed to decode JSON"
	errNoECPoints    = "no EC points in request body"
	errDecodeECPoint = "failed to decode EC point"
	errParseECPoint  = "failed to parse EC point"
)

type epoch uint8

type randRequest struct {
	Points []string `json:"points"`
}

// The response has the same format as the request.
type randResponse randRequest

// Embed an zero-length struct to mark our wrapped structs `noCopy`
//
// Wrapper types should have a corresponding finalizer attached to
// handle releasing the underlying pointer.
//
// NOTE Memory allocated by the Rust library MUST be returned over
// the ffi interface for release. It is critical that no calls to
// free any such pointers are made on the go side. To help enforce
// this, wrappers include an empty member with dummy Lock()/Unlock()
// methods to trigger the mutex copy error in `go vet`.
//
// See https://github.com/golang/go/issues/8005 for further discussion.
type noCopy struct{}

func (*noCopy) Lock()   {}
func (*noCopy) Unlock() {}

// Server represents a PPOPRF randomness server instance.
type Server struct {
	sync.Mutex // TODO: Do we really need a mutex?
	raw        *C.RandomnessServer
	noCopy     noCopy //nolint:structcheck
}

func serverFinalizer(server *Server) {
	C.randomness_server_release(server.raw)
	server.raw = nil
}

// NewServer returns a new PPOPRF randomness server instance.
//
// FIXME Pass in a list of 8-bit tags defining epochs.
// The instance will generate its own secret key.
func NewServer() (*Server, error) {
	// FIXME should we runtime.LockOSThread() here?
	raw := C.randomness_server_create()
	if raw == nil {
		return nil, errors.New("failed to create randomness server")
	}
	server := &Server{raw: raw}
	runtime.SetFinalizer(server, serverFinalizer)
	return server, nil
}

// getEpoch returns the ISO week number of the given timestamp.
func getEpoch(t time.Time) epoch {
	_, week := t.ISOWeek()
	return epoch(week)
}

// getRandomnessHandler returns an http.HandlerFunc so that we can pass our
// server object into
func getRandomnessHandler(srv *Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req randRequest
		var resp randResponse
		var input []byte
		var verifiable bool = false
		var output [32]byte
		var md uint8 = 0

		body, err := ioutil.ReadAll(r.Body)
		if err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
		if len(body) == 0 {
			http.Error(w, errNoReqBody, http.StatusBadRequest)
			return
		}
		if err := json.NewDecoder(bytes.NewReader(body)).Decode(&req); err != nil {
			http.Error(w, errBadJSON, http.StatusBadRequest)
			return
		}
		if len(req.Points) == 0 {
			http.Error(w, errNoECPoints, http.StatusBadRequest)
			return
		}

		for _, encodedPoint := range req.Points {
			// Remove layer of hexadecimal encoding from marshalled EC point.
			marshalledPoint, err := hex.DecodeString(encodedPoint)
			if err != nil {
				http.Error(w, errDecodeECPoint, http.StatusBadRequest)
				return
			}

			// Check if we can parse the given EC point.  If it's un-parseable,
			// we don't need to bother passing the point over our FFI.
			var p ristretto.Point
			if err := p.UnmarshalBinary(marshalledPoint); err != nil {
				http.Error(w, errParseECPoint, http.StatusBadRequest)
				return
			}

			input = []byte(marshalledPoint)
			C.randomness_server_eval(srv.raw,
				(*C.uint8_t)(unsafe.Pointer(&input[0])),
				(C.ulong)(md),
				(C.bool)(verifiable),
				(*C.uint8_t)(unsafe.Pointer(&output[0])))
			resp.Points = append(resp.Points, fmt.Sprintf("%x", output))
		}

		if err := json.NewEncoder(w).Encode(resp); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
	}
}

func main() {
	srv, err := NewServer()
	if err != nil {
		elog.Fatalf("Failed to create randomness server: %s", err)
	}
	elog.Println("Started randomness server.")

	enclave := nitro.NewEnclave(
		&nitro.Config{
			SOCKSProxy: "socks5://127.0.0.1:1080",
			FQDN:       "nitro.nymity.ch",
			Port:       8080,
			Debug:      false,
			UseACME:    false,
		},
	)
	enclave.AddRoute(http.MethodGet, "/randomness", getRandomnessHandler(srv))
	if err := enclave.Start(); err != nil {
		elog.Fatalf("Enclave terminated: %v", err)
	}
}
