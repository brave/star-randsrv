package main

/*
#cgo LDFLAGS: -L target/release -lstar_ppoprf_ffi -lpthread -ldl -static
#cgo CFLAGS: -I include -O3
#include "ppoprf.h"
*/
import "C"

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"errors"
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
	_ "github.com/brave-experiments/nitriding/randseed"

	"github.com/brave-experiments/nitriding"
	"github.com/bwesterb/go-ristretto"
)

var (
	elog              = log.New(os.Stderr, "star-randsrv: ", log.Ldate|log.Ltime|log.LUTC|log.Lshortfile)
	errNoReqBody      = "no request body"
	errBadJSON        = "failed to decode JSON"
	errNoECPoints     = "no EC points in request body"
	errDecodeECPoint  = "failed to decode EC point"
	errParseECPoint   = "failed to parse EC point"
	errEpochExhausted = "epochs are exhausted"
)

const (
	firstEpochTimestamp    = "2022-01-01T00:00:00.000Z"
	defaultEpochLen        = time.Hour * 24 * 7
	serializedPkBufferSize = 16384
	// The last epoch, before our counter overflows
	maxEpoch = ^epoch(0)
)

type epoch uint8

type cliRandRequest struct {
	Points []string `json:"points"`
}

// The response has the same format as the request.
type srvRandResponse cliRandRequest

// The server's response to 'GET /info' requests.
type srvInfoResponse struct {
	PublicKey     string `json:"publicKey"`
	CurrentEpoch  epoch  `json:"currentEpoch"`
	NextEpochTime string `json:"nextEpochTime"`
}

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
	sync.Mutex
	raw    *C.RandomnessServer
	noCopy noCopy //nolint:structcheck
	md     epoch
	done   chan bool
	pubKey string // Base64-encoded public key.
}

// epochLoop periodically punctures the randomness server's PPOPRF and -- if
// necessary -- re-creates the randomness server instance.
func (srv *Server) epochLoop(epochLen time.Duration) {
	ticker := time.NewTicker(epochLen)
	elog.Println("Starting epoch loop.")
	for {
		select {
		case <-srv.done:
			return
		case <-ticker.C:
			if err := srv.puncture(); err != nil {
				if err.Error() == errEpochExhausted {
					if err := srv.init(); err != nil {
						elog.Fatal("Failed to re-initialize randomness server.")
					}
				}
			}
		}
	}
}

// init (re-)initializes the randomness server instance of the Rust FFI.
func (srv *Server) init() error {
	srv.Lock()
	defer srv.Unlock()

	// FIXME should we runtime.LockOSThread() here?
	raw := C.randomness_server_create()
	if raw == nil {
		return errors.New("failed to create randomness server")
	}
	srv.raw = raw

	var pkOutput [serializedPkBufferSize]byte
	pkSize := C.randomness_server_get_public_key(
		srv.raw, (*C.uint8_t)(unsafe.Pointer(&pkOutput[0])))
	if pkSize == 0 {
		return errors.New("failed to get public key")
	}
	srv.pubKey = base64.StdEncoding.EncodeToString(pkOutput[:pkSize])

	elog.Println("(Re-)initialized server instance.")

	return nil
}

// puncture punctures the randomness server's PPOPRF.  As part of the
// puncturing, we're incrementing our epoch counter.  If we're about to exhaust
// our counter (i.e., an integer overflow is about to happen), we return an
// error, which signals to the caller that it's time to create a new randomness
// server instance.
func (srv *Server) puncture() error {
	srv.Lock()
	defer srv.Unlock()

	C.randomness_server_puncture(srv.raw, (C.uint8_t)(srv.md))

	// An epoch is exhausted when our 8-bit counter is about to overflow.
	if srv.md == maxEpoch {
		return errors.New(errEpochExhausted)
	}
	elog.Printf("Punctured epoch %d.", srv.md)
	srv.md++
	return nil
}

func serverFinalizer(server *Server) {
	C.randomness_server_release(server.raw)
	server.raw = nil
}

// NewServer returns a new PPOPRF randomness server instance.
//
// FIXME Pass in a list of 8-bit tags defining epochs.
// The instance will generate its own secret key.
func NewServer(epochLen time.Duration) (*Server, error) {
	server := &Server{
		done: make(chan bool),
	}
	if err := server.init(); err != nil {
		return nil, err
	}
	runtime.SetFinalizer(server, serverFinalizer)
	go server.epochLoop(epochLen)

	return server, nil
}

// getEpoch takes as input 1) the time at which we begin counting epochs and 2)
// the current time.  The function then returns 1) the 8-bit epoch number for
// the current time and 2) the time at which the next epoch begins.
func getEpoch(firstEpochTime time.Time, refTime time.Time) (epoch, time.Time) {
	epochLenSec := int64(defaultEpochLen.Seconds())

	currentSecondsSinceFirstEpoch := refTime.Unix() - firstEpochTime.Unix()
	epochsSinceFirstEpoch := currentSecondsSinceFirstEpoch / epochLenSec
	nextEpochTime := time.Unix(firstEpochTime.Unix()+
		(epochLenSec*(epochsSinceFirstEpoch+1)), 0)
	nextEpochTime = nextEpochTime.In(time.UTC)
	currentEpoch := epochsSinceFirstEpoch % 256
	return epoch(currentEpoch), nextEpochTime
}

// getServerInfo returns an http.HandlerFunc that returns the current epoch
// info and public key to the client.
func getServerInfo(srv *Server) http.HandlerFunc {
	firstEpochTime, _ := time.Parse(time.RFC3339, firstEpochTimestamp)
	return func(w http.ResponseWriter, r *http.Request) {
		currentEpoch, nextEpochTime := getEpoch(firstEpochTime, time.Now())
		resp := srvInfoResponse{
			PublicKey:     srv.pubKey,
			CurrentEpoch:  currentEpoch,
			NextEpochTime: nextEpochTime.Format(time.RFC3339),
		}
		if err := json.NewEncoder(w).Encode(resp); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
	}
}

// getRandomnessHandler returns an http.HandlerFunc so that we can pass our
// server object into.
func getRandomnessHandler(srv *Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		var req cliRandRequest
		var resp srvRandResponse
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
			// Remove layer of base64 encoding from marshalled EC point.
			marshalledPoint, err := base64.StdEncoding.DecodeString(encodedPoint)
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
				(C.uint8_t)(md),
				(C.bool)(verifiable),
				(*C.uint8_t)(unsafe.Pointer(&output[0])))
			resp.Points = append(resp.Points, base64.StdEncoding.EncodeToString(output[:]))
		}

		if err := json.NewEncoder(w).Encode(resp); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
	}
}

func main() {
	srv, err := NewServer(defaultEpochLen)
	if err != nil {
		elog.Fatalf("Failed to create randomness server: %s", err)
	}
	elog.Println("Started randomness server.")

	enclave := nitriding.NewEnclave(
		&nitriding.Config{
			SOCKSProxy: "socks5://127.0.0.1:1080",
			FQDN:       "nitro.nymity.ch",
			Port:       8080,
			Debug:      false,
			UseACME:    false,
		},
	)
	enclave.AddRoute(http.MethodGet, "/randomness", getRandomnessHandler(srv))
	enclave.AddRoute(http.MethodGet, "/info", getServerInfo(srv))
	if err := enclave.Start(); err != nil {
		elog.Fatalf("Enclave terminated: %v", err)
	}
}
