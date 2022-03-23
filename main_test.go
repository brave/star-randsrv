package main

import (
	"fmt"
	"io/ioutil"
	"log"
	"net/http"
	"net/http/httptest"
	"net/url"
	"regexp"
	"strings"
	"testing"
	"time"
)

var (
	// A valid EC point consists of 64 hex digits.
	validPointRegexp = regexp.MustCompile(`^[0-9a-f]{64}$`)
	validPoint       = "f6414bfccc156551d641260ce403992c5d5b0976aca8a72541fda40e8337d867"
	oneWeek          = time.Hour * 24 * 7
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
	req := httptest.NewRequest(http.MethodGet, "/randomness", nil)

	// Call the right endpoint but don't provide an argument key.
	code, resp = makeReq(getRandomnessHandler, req)
	if resp != errNoECPoint {
		t.Errorf("Expected %q but got %q.", errNoECPoint, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Provide the right argument key, but use an invalid value.
	req.URL = u("/randomness?ec_point=foo")
	code, resp = makeReq(getRandomnessHandler, req)
	if resp != errDecodeECPoint {
		t.Errorf("Expected %q but got %q.", errDecodeECPoint, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Provide a decodable argument value, but use an invalid EC point.
	req.URL = u("/randomness?ec_point=deadbeef")
	code, resp = makeReq(getRandomnessHandler, req)
	if resp != errParseECPoint {
		t.Errorf("Expected %q but got %q.", errParseECPoint, resp)
	}
	if code != http.StatusBadRequest {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusBadRequest, code)
	}

	// Finally, show mercy and make a valid request.
	req.URL = u(fmt.Sprintf("/randomness?ec_point=%s", validPoint))
	code, resp = makeReq(getRandomnessHandler, req)
	if !validPointRegexp.MatchString(resp) {
		t.Errorf("Server's response (%q) doesn't look like a valid point.", resp)
	}
	if code != http.StatusOK {
		t.Errorf("Expected HTTP code %d but got %d.", http.StatusOK, code)
	}

	// Ensure that the server is case insensitive.
	req.URL = u(fmt.Sprintf("/randomness?ec_point=%s", strings.ToUpper(validPoint)))
	code, resp = makeReq(getRandomnessHandler, req)
	if !validPointRegexp.MatchString(resp) {
		t.Errorf("Server's response (%q) doesn't look like a valid point.", resp)
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
