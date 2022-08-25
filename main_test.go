package main

import (
	"encoding/json"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

var (
	// A valid EC point consists of base64.
	validPoint = "gpfxPFUTfJvKdD6x5G74VD9Bxdb3efsHYJN0d7vu0XE="
	// Generated random Ristretto points as follows:
	//   var p ristretto.Point
	//   p.Rand()
	//   fmt.Printf("%x\n", p.Bytes())
	validPayload = `{"points": [
		"kKqpcTYWYHrteg62hVEcWGLkw6L+zyGnSLzlszB3SS4=",
		"pOC5TSyy2TrDl8qvC7F5giT77CnaTrzmzRNNOXDS3g4=",
		"gpfxPFUTfJvKdD6x5G74VD9Bxdb3efsHYJN0d7vu0XE="
	]}`
)

func srvWithEpochLen(epochLen time.Duration) *Server {
	srv, err := NewServer(defaultFirstEpochTime, epochLen)
	srv.epochLen = epochLen
	if err != nil {
		log.Fatalf("Failed to create randomness server: %s", err)
	}
	return srv
}

// Pass a request to to given hander and return the status and response body.
func makeReq(handler http.HandlerFunc, req *http.Request) (int, string) {
	rec := httptest.NewRecorder()
	handler(rec, req)

	res := rec.Result()
	defer res.Body.Close()

	data, err := io.ReadAll(res.Body)
	if err != nil {
		log.Fatalf("Failed to read HTTP response body: %s", err)
	}
	return res.StatusCode, strings.TrimSpace(string(data))
}

// Make an info request
func makeInfoReq(srv *Server) srvInfoResponse {
	handler := getServerInfo(srv)

	var res srvInfoResponse
	req := httptest.NewRequest(http.MethodPost, "/info", nil)
	status, result := makeReq(handler, req)
	if status != http.StatusOK {
		log.Fatalf("Expected HTTP code %d but got %d.", http.StatusOK, status)
	}
	if err := json.NewDecoder(strings.NewReader(result)).Decode(&res); err != nil {
		log.Fatalf("Failed to unmarshal server's JSON response: %s", err)
	}

	return res
}

// Make a randomness request
//
// makeRandomnessReq() makes a request without specifying an epoch.
// makeRandomnessReq(epoch) makes a request with the given epoch.
func makeRandomnessReq(srv *Server, params ...epoch) srvRandResponse {
	var payload string
	if len(params) == 0 {
		payload = fmt.Sprintf(`{ "points": [ "%s" ] }`, validPoint)
	} else if len(params) == 1 {
		payload = fmt.Sprintf(`{ "points": [ "%s" ], "epoch": %d }`, validPoint, params[0])
	} else {
		log.Fatalf("Invalid number of arguments (%d) to makeRandomnessReq", len(params))
	}

	var res srvRandResponse
	handler := getRandomnessHandler(srv)
	req := httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader(payload))
	status, result := makeReq(handler, req)
	if status != http.StatusOK {
		log.Fatalf("Expected HTTP code %d but got %d.", http.StatusOK, status)
	}
	if err := json.NewDecoder(strings.NewReader(result)).Decode(&res); err != nil {
		log.Fatalf("Failed to unmarshal server's JSON response: %s", err)
	}

	return res
}

func TestEpochRotation(t *testing.T) {
	var origMd, newMd epoch
	srv := srvWithEpochLen(time.Millisecond)
	origMd, _ = srv.getEpoch(time.Now().UTC())
	// Sleep until the server had a chance to switch epochs.
	time.Sleep(time.Millisecond * 10)
	newMd, _ = srv.getEpoch(time.Now().UTC())

	if origMd == newMd {
		t.Fatal("Expected epoch rotation but md values are identical.")
	}
}

func TestPubKeyRotation(t *testing.T) {
	var pubKey1, pubKey2 [serializedPkBufferSize]byte
	srv := srvWithEpochLen(defaultEpochLen)
	copy(pubKey1[:], srv.pubKey)

	// Re-initialize the randomness server, which will result in a new (and
	// therefore different) public key.
	_ = srv.init()
	copy(pubKey2[:], srv.pubKey)

	if pubKey1 == pubKey2 {
		t.Fatal("Public keys are identical despite re-initializing server.")
	}
}

func TestPuncture(t *testing.T) {
	var err error
	srv := srvWithEpochLen(defaultEpochLen)

	for i := epoch(0); i < maxEpoch; i++ {
		if err = srv.puncture(i); err != nil {
			t.Fatalf("Failed to puncture epoch: %s", err)
		}
	}
	// The next call should result in an errEpochExhausted.
	err = srv.puncture(maxEpoch)
	if err.Error() != errEpochExhausted {
		t.Fatalf("Expected error %q but got %q.", errEpochExhausted, err)
	}
}

func TestEpoch(t *testing.T) {
	var e epoch
	var nextEpochTime time.Time
	srv := srvWithEpochLen(defaultEpochLen)
	refTime := defaultFirstEpochTime

	for i := 0; i <= 500; i++ {
		e, nextEpochTime = srv.getEpoch(refTime)
		if e != epoch(i) {
			t.Fatalf("Expected epoch %d but got %d for timestamp %s.", epoch(i), e, refTime)
		}
		refTime = refTime.Add(defaultEpochLen)
		if nextEpochTime != refTime {
			t.Fatalf("Expected next epoch timestamp %s but got %s.", refTime, nextEpochTime)
		}
	}
}

func TestInfoContentType(t *testing.T) {
	req := httptest.NewRequest(http.MethodGet, "/info", nil)
	handler := getServerInfo(srvWithEpochLen(defaultEpochLen))

	rec := httptest.NewRecorder()
	handler(rec, req)
	res := rec.Result()
	defer res.Body.Close()

	if res.StatusCode != http.StatusOK {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusOK, res.StatusCode)
	}
	if res.Header.Get(httpContentType) != contentTypeJSON {
		t.Errorf("Expected %q but got %q.", contentTypeJSON, res.Header.Get("Content-Type"))
	}
}

func TestRandomnessContentType(t *testing.T) {
	req := httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader(validPayload))
	handler := getRandomnessHandler(srvWithEpochLen(defaultEpochLen))

	rec := httptest.NewRecorder()
	handler(rec, req)
	res := rec.Result()
	defer res.Body.Close()

	if res.StatusCode != http.StatusOK {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusOK, res.StatusCode)
	}
	if res.Header.Get(httpContentType) != contentTypeJSON {
		t.Errorf("Expected %q but got %q.", contentTypeJSON, res.Header.Get("Content-Type"))
	}
}

func TestRandomnessEpoch(t *testing.T) {
	srv := srvWithEpochLen(defaultEpochLen)

	// Fetch the server's current epoch to test against.
	currentEpoch := makeInfoReq(srv).CurrentEpoch

	// Submit a point without specifying an epoch.
	noEpochResponse := makeRandomnessReq(srv)
	if noEpochResponse.Epoch == nil {
		t.Fatalf("Expected randomness response to include an epoch")
	}
	if *noEpochResponse.Epoch != currentEpoch {
		t.Errorf("Expected epoch %d but got %d.", currentEpoch, *noEpochResponse.Epoch)
	}

	// Explicitly request the current epoch and verify it's used.
	currentEpochResponse := makeRandomnessReq(srv, currentEpoch)
	if currentEpochResponse.Epoch == nil {
		t.Fatalf("Expected randomness response to include an epoch")
	}
	if *currentEpochResponse.Epoch != currentEpoch {
		t.Errorf("Expected epoch %d but got %d.", currentEpoch, *currentEpochResponse.Epoch)
	}

	// Request a future epoch.
	futureEpochResponse := makeRandomnessReq(srv, currentEpoch+1)
	if futureEpochResponse.Epoch == nil {
		t.Fatalf("Expected randomness response to include an epoch")
	}
	if *futureEpochResponse.Epoch != currentEpoch+1 {
		t.Errorf("Expected epoch %d but got %d.", currentEpoch+1, *futureEpochResponse.Epoch)
	}
}

func TestHTTPHandler(t *testing.T) {
	var resp string
	var code int
	validReq := httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader(validPayload))
	handler := getRandomnessHandler(srvWithEpochLen(defaultEpochLen))

	// Call the right endpoint but don't provide a request body.
	emptyReq := httptest.NewRequest(http.MethodPost, "/randomness", nil)
	code, resp = makeReq(handler, emptyReq)
	if resp != errNoReqBody {
		t.Errorf("Expected %q but got %q.", errNoReqBody, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Provide a request body, but have it be nonsense.
	badReq := httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader("foo"))
	code, resp = makeReq(handler, badReq)
	if resp != errBadJSON {
		t.Errorf("Expected %q but got %q.", errBadJSON, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Provide valid JSON but use a bogus EC point.
	badReq = httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader(`{"points":["foo"]}`))
	code, resp = makeReq(handler, badReq)
	if resp != errDecodeECPoint {
		t.Errorf("Expected %q but got %q.", errDecodeECPoint, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Provide an invalid EC point.
	badPayload := `{"points":["1111111111111111111111111111111111111111111111111111111111111111"]}`
	badReq = httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader(badPayload))
	code, resp = makeReq(handler, badReq)
	if resp != errParseECPoint {
		t.Errorf("Expected %q but got %q.", errParseECPoint, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Finally, show mercy and make a valid request.
	code, resp = makeReq(handler, validReq)
	var r srvRandResponse
	if err := json.NewDecoder(strings.NewReader(resp)).Decode(&r); err != nil {
		t.Errorf("Failed to unmarshal server's JSON response: %s", err)
	}
	if code != http.StatusOK {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusOK, code)
	}
}

func TestEpochNilPointer(t *testing.T) {
	c := cliRandRequest{}

	// Client didn't provide an epoch value.  Epoch must be nil.
	blob := []byte(`{"points": ["foo", "bar"]}`)
	if err := json.Unmarshal(blob, &c); err != nil {
		t.Fatalf("Error while unmarshalling: %v", err)
	}
	if c.Epoch != nil {
		t.Fatal("Expected epoch to be nil but it's not.")
	}
}

func TestEpochNonNilPointer(t *testing.T) {
	c := cliRandRequest{}

	// Client did provide an epoch value.  Epoch must not be nil.
	blob := []byte(`{"points": ["foo", "bar"], "epoch": 123}`)
	if err := json.Unmarshal(blob, &c); err != nil {
		t.Fatalf("Error while unmarshalling: %v", err)
	}
	if c.Epoch == nil {
		t.Fatal("Expected epoch to be non-nil but it's nil.")
	}
}

func BenchmarkHTTPHandler(b *testing.B) {
	req := httptest.NewRequest(http.MethodGet, fmt.Sprintf("/randomness?ec_point=%s", validPoint), nil)
	handler := getRandomnessHandler(srvWithEpochLen(defaultEpochLen))

	for n := 0; n < b.N; n++ {
		_, _ = makeReq(handler, req)
	}
}
