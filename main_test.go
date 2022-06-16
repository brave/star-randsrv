package main

import (
	"encoding/json"
	"fmt"
	"io/ioutil"
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
)

func makeReq(getHandler func(*Server) http.HandlerFunc, req *http.Request) (int, string) {
	var handler http.HandlerFunc
	srv, err := NewServer(defaultEpochLen)
	if err != nil {
		log.Fatalf("Failed to create randomness server: %s", err)
	}
	handler = getHandler(srv)

	rec := httptest.NewRecorder()
	handler(rec, req)

	res := rec.Result()
	defer res.Body.Close()

	data, err := ioutil.ReadAll(res.Body)
	if err != nil {
		log.Fatalf("Failed to read HTTP response body: %s", err)
	}
	return res.StatusCode, strings.TrimSpace(string(data))
}

func TestEpochLoop(t *testing.T) {
	var origMd, newMd uint8
	srv, _ := NewServer(time.Millisecond)
	origMd = srv.md
	// Sleep until the server had a chance to switch epochs.
	time.Sleep(time.Millisecond * 10)
	newMd = srv.md

	if origMd == newMd {
		t.Fatal("Expected epoch rotation but md values are identical.")
	}
}

func TestPubKeyRotation(t *testing.T) {
	var pubKey1, pubKey2 [serializedPkBufferSize]byte
	srv, _ := NewServer(defaultEpochLen)
	copy(pubKey1[:], srv.pubKey)

	// Re-initialize the randomness server, which will result in a new (and
	// therefore different) public key.
	srv.init()
	copy(pubKey2[:], srv.pubKey)

	if pubKey1 == pubKey2 {
		t.Fatal("Public keys are identical despite re-initializing server.")
	}
}

func TestPuncture(t *testing.T) {
	var err error
	srv, _ := NewServer(defaultEpochLen)

	for i := uint8(0); i < maxEpoch; i++ {
		if err = srv.puncture(); err != nil {
			t.Fatalf("Failed to puncture epoch: %s", err)
		}
	}
	// The next call should result in an errEpochExhausted.
	err = srv.puncture()
	if err.Error() != errEpochExhausted {
		t.Fatalf("Expected error %q but got %q.", errEpochExhausted, err)
	}
}

func TestEpoch(t *testing.T) {
	var ts time.Time
	var e epoch
	var nextEpochTime time.Time
	// Jan 1, 2022, the first epoch
	ts, _ = time.Parse(time.RFC3339, "2022-01-01T00:00:00Z")

	firstEpochTime, _ := time.Parse(time.RFC3339, firstEpochTimestamp)

	for i := 0; i <= 500; i++ {
		e, nextEpochTime = getEpoch(firstEpochTime, ts)
		if e != epoch(i) {
			t.Errorf("Expected epoch %d but got %d for ts %s.", epoch(i), e, ts)
		}
		ts = ts.Add(defaultEpochLen)
		if nextEpochTime != ts {
			t.Errorf("Expected next epoch timestamp %s but got %s.",
				ts.Format(time.RFC3339Nano), nextEpochTime)
		}
	}
}

func TestHTTPHandler(t *testing.T) {
	var resp string
	var code int
	// Generated random Ristretto points as follows:
	//   var p ristretto.Point
	//   p.Rand()
	//   fmt.Printf("%x\n", p.Bytes())
	validPayload := `{"points": [
		"kKqpcTYWYHrteg62hVEcWGLkw6L+zyGnSLzlszB3SS4=",
		"pOC5TSyy2TrDl8qvC7F5giT77CnaTrzmzRNNOXDS3g4=",
		"gpfxPFUTfJvKdD6x5G74VD9Bxdb3efsHYJN0d7vu0XE="
	]}`
	validReq := httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader(validPayload))

	// Call the right endpoint but don't provide a request body.
	emptyReq := httptest.NewRequest(http.MethodPost, "/randomness", nil)
	code, resp = makeReq(getRandomnessHandler, emptyReq)
	if resp != errNoReqBody {
		t.Errorf("Expected %q but got %q.", errNoReqBody, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Provide a request body, but have it be nonsense.
	badReq := httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader("foo"))
	code, resp = makeReq(getRandomnessHandler, badReq)
	if resp != errBadJSON {
		t.Errorf("Expected %q but got %q.", errBadJSON, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Provide valid JSON but use a bogus EC point.
	badReq = httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader(`{"points":["foo"]}`))
	code, resp = makeReq(getRandomnessHandler, badReq)
	if resp != errDecodeECPoint {
		t.Errorf("Expected %q but got %q.", errDecodeECPoint, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Provide an invalid EC point.
	badPayload := `{"points":["1111111111111111111111111111111111111111111111111111111111111111"]}`
	badReq = httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader(badPayload))
	code, resp = makeReq(getRandomnessHandler, badReq)
	if resp != errParseECPoint {
		t.Errorf("Expected %q but got %q.", errParseECPoint, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Finally, show mercy and make a valid request.
	code, resp = makeReq(getRandomnessHandler, validReq)
	var r randResponse
	if err := json.NewDecoder(strings.NewReader(resp)).Decode(&r); err != nil {
		t.Errorf("Failed to unmarshal server's JSON response: %s", err)
	}
	if code != http.StatusOK {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusOK, code)
	}
}

func BenchmarkHTTPHandler(b *testing.B) {
	req := httptest.NewRequest(http.MethodGet, fmt.Sprintf("/randomness?ec_point=%s", validPoint), nil)
	for n := 0; n < b.N; n++ {
		_, _ = makeReq(getRandomnessHandler, req)
	}
}
