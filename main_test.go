package main

import (
	"encoding/json"
	"fmt"
	"io/ioutil"
	"log"
	"net/http"
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
	"time"
)

var (
	// A valid EC point consists of 64 hex digits.
	validPoint = "f6414bfccc156551d641260ce403992c5d5b0976aca8a72541fda40e8337d867"
	oneWeek    = time.Hour * 24 * 7
)

func makeReq(getHandler func(*Server) http.HandlerFunc, req *http.Request) (int, string) {
	var handler http.HandlerFunc
	srv, err := NewServer()
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

func u(strURL string) *url.URL {
	u, err := url.Parse(strURL)
	if err != nil {
		log.Fatalf("Failed to parse URL %q: %s", strURL, err)
	}
	return u
}

func TestEpoch(t *testing.T) {
	var ts time.Time
	var e epoch
	// Jan 1, 2020 falls on a Wednesday, so according to the ISO week date
	// system, it's week 1 (as opposed to week 52 or 53 of the previous year).
	ts, _ = time.Parse(time.RFC3339, "2020-01-01T00:00:00Z")

	for i := 1; i <= 52; i++ {
		e = getEpoch(ts)
		if e != epoch(i) {
			t.Errorf("Expected epoch %d but got %d for ts %s.", epoch(i), e, ts)
		}
		ts = ts.Add(oneWeek)
	}

	// Check the edge case of the last second of the year.  2020 is a leap
	// year, so it has 53 weeks.
	ts, _ = time.Parse(time.RFC3339, "2020-12-31T23:59:59Z")
	e = getEpoch(ts)
	if e != epoch(53) {
		t.Errorf("Expected epoch %d but got %d for ts %s.", epoch(53), e, ts)
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
		"90aaa9713616607aed7a0eb685511c5862e4c3a2fecf21a748bce5b33077492e",
		"a4e0b94d2cb2d93ac397caaf0bb1798224fbec29da4ebce6cd134d3970d2de0e",
		"8297f13c55137c9bca743eb1e46ef8543f41c5d6f779fb0760937477bbeed171"
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

	// Ensure that the server is case insensitive.
	upperCase := strings.ToUpper(validPayload)
	upperReq := httptest.NewRequest(http.MethodPost, "/randomness", strings.NewReader(upperCase))
	code, resp = makeReq(getRandomnessHandler, upperReq)
	fmt.Println(resp)
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
