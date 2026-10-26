// Send a fixed, validated workload over fresh loopback HTTP/1.1 connections.
package main

import (
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"math"
	"net/http"
	"os"
	"slices"
	"sort"
	"sync"
	"syscall"
	"time"
)

type Case struct {
	Route, Method, Path, Input, Media, Body string
	Status                                  int
}
type Sample struct {
	route  string
	micros float64
}
type Stats struct {
	Count               int
	P50US, P95US, P99US float64
}
type Result struct {
	Requests                       int
	Seconds, RPS, ClientCPUSeconds float64
	All                            Stats
	ByRoute                        map[string]Stats
}

func summarize(xs []float64) Stats {
	sort.Float64s(xs)
	at := func(q float64) float64 { return xs[int(math.Ceil(float64(len(xs))*q))-1] }
	return Stats{len(xs), at(.5), at(.95), at(.99)}
}

func cpu() float64 {
	var r syscall.Rusage
	if err := syscall.Getrusage(syscall.RUSAGE_SELF, &r); err != nil {
		panic(err)
	}
	return float64(r.Utime.Nano()+r.Stime.Nano()) / 1e9
}

func main() {
	fixtures := flag.String("cases", "", "JSON request fixtures")
	addr := flag.String("addr", "", "loopback host:port")
	profile := flag.String("profile", "mixed", "mixed, health, hello, total, session-new, or session-replay")
	n := flag.Int("n", 0, "requests")
	c := flag.Int("c", 1, "concurrent clients")
	flag.Parse()
	data, err := os.ReadFile(*fixtures)
	if err != nil {
		panic(err)
	}
	var cases []Case
	if err = json.Unmarshal(data, &cases); err != nil {
		panic(err)
	}
	session := *profile == "session-new" || *profile == "session-replay"
	if session {
		cases = []Case{{Route: *profile, Method: "GET", Path: "/visits", Status: 200, Media: "application/json"}}
	} else if *profile != "mixed" {
		cases = slices.DeleteFunc(cases, func(k Case) bool { return k.Route != *profile })
	}
	if len(cases) == 0 || *n < *c || *c < 1 || *n%len(cases) != 0 {
		panic("requests must cover each fixture equally")
	}
	client := http.Client{Transport: &http.Transport{DisableKeepAlives: true, DisableCompression: true}, Timeout: 5 * time.Second}
	out := make([]Sample, *n)
	var wg sync.WaitGroup
	var ready sync.WaitGroup
	startSignal := make(chan struct{})
	for w := 0; w < *c; w++ {
		wg.Add(1)
		ready.Add(1)
		go func(worker int) {
			defer wg.Done()
			var cookie *http.Cookie
			var visits int
			ready.Done()
			<-startSignal
			for i := worker; i < *n; i += *c {
				k := cases[i%len(cases)]
				req, err := http.NewRequest(k.Method, "http://"+*addr+k.Path, bytes.NewBufferString(k.Input))
				if err != nil {
					panic(err)
				}
				req.Close = true
				if k.Method == "POST" {
					req.Header.Set("Content-Type", "application/json")
				}
				if *profile == "session-replay" && cookie != nil {
					req.AddCookie(cookie)
				}
				before := time.Now()
				res, err := client.Do(req)
				if err != nil {
					panic(err)
				}
				body, err := io.ReadAll(res.Body)
				res.Body.Close()
				if err != nil {
					panic(err)
				}
				us := float64(time.Since(before).Nanoseconds()) / 1e3
				want := k.Body
				if session {
					if *profile == "session-replay" {
						visits++
					} else {
						visits = 1
					}
					want = fmt.Sprintf("{\"visits\":%d}\n", visits)
					cookies := res.Cookies()
					if len(cookies) != 1 || cookies[0].Name != "session" || cookies[0].Value == "" {
						panic("signed session cookie missing")
					}
					cookie = cookies[0]
				}
				if res.StatusCode != k.Status || res.Header.Get("Content-Type") != k.Media || string(body) != want {
					panic(fmt.Sprintf("%s %s: %d %q %q", k.Method, k.Path, res.StatusCode, res.Header.Get("Content-Type"), body))
				}
				out[i] = Sample{k.Route, us}
			}
		}(w)
	}
	ready.Wait()
	beforeCPU := cpu()
	start := time.Now()
	close(startSignal)
	wg.Wait()
	sec := time.Since(start).Seconds()
	usedCPU := cpu() - beforeCPU
	all := make([]float64, *n)
	routes := map[string][]float64{}
	for i, s := range out {
		all[i] = s.micros
		routes[s.route] = append(routes[s.route], s.micros)
	}
	report := Result{*n, sec, float64(*n) / sec, usedCPU, summarize(all), map[string]Stats{}}
	for route, xs := range routes {
		report.ByRoute[route] = summarize(xs)
	}
	if err := json.NewEncoder(os.Stdout).Encode(report); err != nil {
		panic(err)
	}
}
