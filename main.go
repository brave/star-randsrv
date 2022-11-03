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
	"flag"
	"fmt"
	"io"
	"log"
	"net/http"
	"os"
	"runtime"
	"sync"
	"time"
	"unsafe"

	// This module must be imported first because of its side effects of
	// seeding our system entropy pool.
	_ "github.com/brave/nitriding/randseed"

	"github.com/brave/nitriding"
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
	errTooManyPoints  = fmt.Sprintf("too many points (> %d) given", maxPoints)

	defaultFirstEpochTime, _ = time.Parse(time.RFC3339, "2022-01-01T00:00:00.000Z")
)

const (
	defaultEpochLen = time.Hour * 24 * 7

	// We need room for 256 epochs (each having a 32-byte key), the base key
	// (32 bytes), the epoch values labelling each key (256 bytes), the epoch
	// count (1-8 bytes), and possibly some buffer lengths.
	serializedPkBufferSize uint = 10240
	// The last epoch, before our counter overflows
	maxEpoch = ^epoch(0)
	// The maximum number of points we're willing to process
	maxPoints = 1000
	// HTTP header keys and values.
	httpContentType = "Content-Type"
	contentTypeJSON = "application/json"
)

type epoch uint8

type cliRandRequest struct {
	Points []string `json:"points"`
	Epoch  *epoch   `json:"epoch"`
}

// The response has the same format as the request.
type srvRandResponse cliRandRequest

// The server's response to 'GET /info' requests.
type srvInfoResponse struct {
	PublicKey     string `json:"publicKey"`
	CurrentEpoch  epoch  `json:"currentEpoch"`
	NextEpochTime string `json:"nextEpochTime"`
	MaxPoints     int    `json:"maxPoints"`
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
	raw            *C.RandomnessServer
	noCopy         noCopy //nolint:structcheck
	pubKey         string // Base64-encoded public key.
	firstEpochTime time.Time
	epochLen       time.Duration
}

// epochLoop periodically punctures the randomness server's PPOPRF and -- if
// necessary -- re-creates the randomness server instance.
func (srv *Server) epochLoop() {
	// Odds are that the server's start time does not coincide with the next
	// epoch's start time.  We therefore wait until the next epoch begins, at
	// which point we begin our epoch rotation loop.
	now := time.Now().UTC()
	currentEpoch, nextEpochTime := srv.getEpoch(now)
	diff := nextEpochTime.Sub(now)
	elog.Printf("Waiting %s until next epoch begins at %s.", diff, nextEpochTime)
	<-time.NewTicker(diff).C

	ticker := time.NewTicker(srv.epochLen)
	elog.Println("Starting epoch loop.")
	for range ticker.C {
		if err := srv.puncture(currentEpoch); err != nil {
			if err.Error() == errEpochExhausted {
				if err := srv.init(); err != nil {
					elog.Fatal("Failed to re-initialize randomness server.")
				}
			}
		}
		currentEpoch, _ = srv.getEpoch(time.Now().UTC())
	}
}

// init (re-)initializes the randomness server instance of the Rust FFI.
func (srv *Server) init() error {
	srv.Lock()
	defer srv.Unlock()

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

// puncture takes an epoch tag, and punctures the randomness server's PPOPRF.
// If we're about to exhaust our counter (i.e., an integer overflow is about to happen),
// we return an error, which signals to the caller that it's time to create a new randomness
// server instance.
func (srv *Server) puncture(md epoch) error {
	srv.Lock()
	defer srv.Unlock()

	C.randomness_server_puncture(srv.raw, (C.uint8_t)(md))

	// An epoch is exhausted when our 8-bit counter is about to overflow.
	if md == maxEpoch {
		return errors.New(errEpochExhausted)
	}
	elog.Printf("Punctured epoch %d.", md)
	return nil
}

func serverFinalizer(server *Server) {
	server.Lock()
	defer server.Unlock()

	C.randomness_server_release(server.raw)
	server.raw = nil
}

// NewServer returns a new PPOPRF randomness server instance.
//
// FIXME Pass in a list of 8-bit tags defining epochs.
// The instance will generate its own secret key.
func NewServer(firstEpochTime time.Time, epochLen time.Duration) (*Server, error) {
	server := &Server{
		firstEpochTime: firstEpochTime,
		epochLen:       epochLen,
	}
	if err := server.init(); err != nil {
		return nil, err
	}
	runtime.SetFinalizer(server, serverFinalizer)
	go server.epochLoop()

	return server, nil
}

// getEpoch takes the reference time used to calculate the epoch.
// The function then returns 1) the 8-bit epoch number for the
// current time and 2) the time at which the next epoch begins.
func (srv *Server) getEpoch(refTime time.Time) (epoch, time.Time) {
	epochLenMs := srv.epochLen.Milliseconds()
	msSinceFirstEpoch := refTime.UnixMilli() - srv.firstEpochTime.UnixMilli()
	if msSinceFirstEpoch < 0 {
		elog.Panicln("getEpoch: refTime is less than firstEpochTime!")
	}
	epochsSinceFirstEpoch := msSinceFirstEpoch / epochLenMs

	nextEpochTime := time.UnixMilli(srv.firstEpochTime.UnixMilli() +
		(epochLenMs * (epochsSinceFirstEpoch + 1)))
	nextEpochTime = nextEpochTime.In(time.UTC)
	curEpoch := epochsSinceFirstEpoch % 256
	return epoch(curEpoch), nextEpochTime
}

// getFirstEpochTimeAndLen retrieves the first epoch time and epoch length
// from command-line flags, if available. If flags are not present, defaults
// will be returned.
func getFirstEpochTimeAndLen() (time.Time, time.Duration) {
	testEpoch := flag.Int("test-epoch", -1, "Epoch to use for testing")
	epochLenSec := flag.Int(
		"test-epoch-len",
		0,
		"Length of each epoch for testing (seconds)",
	)
	flag.Parse()
	firstEpochTime := defaultFirstEpochTime
	epochLen := defaultEpochLen
	if *epochLenSec > 0 {
		epochLen = time.Duration(*epochLenSec) * time.Second
	}
	if *testEpoch >= 0 && *testEpoch <= 255 {
		firstEpochTime = time.Unix(time.Now().UTC().Unix()-
			(int64(epochLen.Seconds())*int64(*testEpoch)), 0)
	}
	return firstEpochTime, epochLen
}

// getServerInfo returns an http.HandlerFunc that returns the current epoch
// info and public key to the client.
func getServerInfo(srv *Server) http.HandlerFunc {
	return func(w http.ResponseWriter, r *http.Request) {
		currentEpoch, nextEpochTime := srv.getEpoch(time.Now().UTC())
		srv.Lock()
		resp := srvInfoResponse{
			PublicKey:     srv.pubKey,
			CurrentEpoch:  currentEpoch,
			NextEpochTime: nextEpochTime.Format(time.RFC3339),
			MaxPoints:     maxPoints,
		}
		srv.Unlock()
		w.Header().Set(httpContentType, contentTypeJSON)
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

		body, err := io.ReadAll(r.Body)
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
		if len(req.Points) > maxPoints {
			http.Error(w, errTooManyPoints, http.StatusBadRequest)
			return
		}
		if req.Epoch == nil {
			// Default to the current epoch since none was specifed.
			currentEpoch, _ := srv.getEpoch(time.Now().UTC())
			req.Epoch = new(epoch)
			*req.Epoch = currentEpoch
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
			srv.Lock()
			evalRes := C.randomness_server_eval(srv.raw,
				(*C.uint8_t)(unsafe.Pointer(&input[0])),
				(C.uint8_t)(*req.Epoch),
				(C.bool)(verifiable),
				(*C.uint8_t)(unsafe.Pointer(&output[0])))
			srv.Unlock()

			if !evalRes {
				http.Error(w, "Randomness eval failed", http.StatusInternalServerError)
				return
			}

			resp.Points = append(resp.Points, base64.StdEncoding.EncodeToString(output[:]))
			resp.Epoch = req.Epoch
		}

		w.Header().Set(httpContentType, contentTypeJSON)
		if err := json.NewEncoder(w).Encode(resp); err != nil {
			http.Error(w, err.Error(), http.StatusInternalServerError)
			return
		}
	}
}

func main() {
	elog.Printf("Running as UID %d.", os.Getuid())
	firstEpochTime, epochLen := getFirstEpochTimeAndLen()
	srv, err := NewServer(firstEpochTime, epochLen)
	if err != nil {
		elog.Fatalf("Failed to create randomness server: %s", err)
	}
	elog.Println("Started randomness server.")

	enclave := nitriding.NewEnclave(
		&nitriding.Config{
			SOCKSProxy: "socks5://127.0.0.1:1080",
			FQDN:       "star-randsrv.bsg.brave.software",
			Port:       8443,
			Debug:      false,
			UseACME:    true,
		},
	)
	enclave.AddRoute(http.MethodPost, "/randomness", getRandomnessHandler(srv))
	enclave.AddRoute(http.MethodGet, "/info", getServerInfo(srv))

	if err := enclave.Start(); err != nil {
		elog.Fatalf("Enclave terminated: %v", err)
	}
}
