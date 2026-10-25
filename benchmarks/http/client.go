package main

import (
	"bufio"
	"bytes"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net"
	"os"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
)

type sample struct {
	N                   int
	Tail                []float64
	Err                 string
	Scheduled, Dispatch []float64
	OnTime              int
}

func worker(addr string, closed bool, depth, padding int, start, end time.Time, id, workers, rate, slots int) sample {
	var s sample
	var c net.Conn
	var reader *bufio.Reader
	defer func() {
		if c != nil {
			c.Close()
		}
	}()
	mode := "keep-alive"
	if closed {
		mode = "close"
	}
	req := []byte("GET /hello HTTP/1.1\r\nHost: localhost\r\nConnection: " + mode + "\r\n\r\n")
	if padding > 0 {
		req = []byte("GET /hello HTTP/1.1\r\nHost: localhost\r\nX-Padding: " + strings.Repeat("x", padding) + "\r\nConnection: " + mode + "\r\n\r\n")
	}
	req = bytes.Repeat(req, depth)
	body := make([]byte, 6)
	fail := func(e error) sample { s.Err = e.Error(); return s }
	for seq := id; ; seq += workers {
		var due time.Time
		if rate > 0 {
			if seq >= slots {
				break
			}
			due = start.Add(time.Duration(int64(seq) * int64(time.Second) / int64(rate)))
			if delay := time.Until(due); delay > 0 {
				time.Sleep(delay)
			}
		} else if !time.Now().Before(end) {
			break
		}
		begin := time.Now()
		if c == nil {
			var e error
			c, e = net.DialTimeout("tcp4", addr, 2*time.Second)
			if e != nil {
				return fail(e)
			}
			c.(*net.TCPConn).SetNoDelay(true)
			reader = bufio.NewReader(c)
		}
		c.SetDeadline(time.Now().Add(4 * time.Second))
		if _, e := c.Write(req); e != nil {
			return fail(e)
		}
		for k := 0; k < depth; k++ {
			status, e := reader.ReadString('\n')
			if e != nil {
				return fail(e)
			}
			if !strings.HasPrefix(status, "HTTP/1.1 200 ") {
				return fail(fmt.Errorf("non-200 %q", status))
			}
			size := -1
			for {
				line, e := reader.ReadString('\n')
				if e != nil {
					return fail(e)
				}
				if line == "\r\n" {
					break
				}
				key, v, ok := strings.Cut(line, ":")
				if !ok {
					return fail(fmt.Errorf("malformed %q", line))
				}
				if strings.EqualFold(key, "content-length") {
					size, e = strconv.Atoi(strings.TrimSpace(v))
					if e != nil {
						return fail(e)
					}
				}
			}
			if size != 6 {
				return fail(fmt.Errorf("content length %d", size))
			}
			if _, e = io.ReadFull(reader, body); e != nil {
				return fail(e)
			}
			if !bytes.Equal(body, []byte("hello\n")) {
				return fail(fmt.Errorf("wrong response %q", body))
			}
			if rate > 0 {
				completed := time.Now()
				s.Scheduled = append(s.Scheduled, float64(completed.Sub(due).Nanoseconds())/1000)
				s.Dispatch = append(s.Dispatch, float64(begin.Sub(due).Nanoseconds())/1000)
				if !completed.After(end) {
					s.OnTime++
				}
			}
			if s.N%7 == 0 {
				s.Tail = append(s.Tail, float64(time.Since(begin).Nanoseconds())/1000)
			}
			s.N++
		}
		if closed {
			c.Close()
			c = nil
		}
	}
	return s
}
func main() {
	n := flag.Int("c", 1, "connections")
	depth := flag.Int("pipeline", 1, "requests per batch")
	dur := flag.Duration("duration", 3*time.Second, "measurement duration")
	addr := flag.String("addr", "127.0.0.1:8000", "endpoint")
	closed := flag.Bool("close", false, "new connection for every request")
	padding := flag.Int("request-padding", 0, "bytes in X-Padding")
	rate := flag.Int("rate", 0, "fixed aggregate arrivals/second (zero measures saturation)")
	flag.Parse()
	if *n < 1 || *dur <= 0 || *rate < 0 || (*rate > 0 && (*closed || *depth != 1)) {
		panic("invalid fixed-rate configuration")
	}
	slots := int(dur.Seconds() * float64(*rate))
	if *rate > 0 && slots < *n {
		panic("schedule must cover all connections")
	}
	if *depth < 1 || (*closed && *depth != 1) {
		panic("invalid pipeline depth")
	}
	start := time.Now()
	if *rate > 0 {
		start = start.Add(50 * time.Millisecond)
	}
	end := start.Add(*dur)
	ch := make(chan sample, *n)
	var wg sync.WaitGroup
	for i := 0; i < *n; i++ {
		wg.Add(1)
		go func(id int) {
			defer wg.Done()
			ch <- worker(*addr, *closed, *depth, *padding, start, end, id, *n, *rate, slots)
		}(i)
	}
	wg.Wait()
	elapsed := time.Since(start).Seconds()
	close(ch)
	total := 0
	var tail []float64
	var perWorker []int
	var scheduled, dispatch []float64
	onTime := 0
	for s := range ch {
		if s.Err != "" {
			fmt.Fprintln(os.Stderr, s.Err)
			os.Exit(1)
		}
		if s.N < 1 {
			panic("starved worker")
		}
		total += s.N
		scheduled = append(scheduled, s.Scheduled...)
		dispatch = append(dispatch, s.Dispatch...)
		onTime += s.OnTime
		perWorker = append(perWorker, s.N)
		tail = append(tail, s.Tail...)
	}
	sort.Ints(perWorker)
	if total < 1 {
		panic("no completed requests")
	}
	sort.Float64s(tail)
	p := func(q float64) float64 { return tail[int(float64(len(tail)-1)*q)] }
	out := map[string]any{"requests": total, "seconds": elapsed, "rps": float64(total) / elapsed, "p50_us": p(.50), "p95_us": p(.95), "p99_us": p(.99), "latency_samples": len(tail), "errors": 0, "worker_requests": perWorker}
	if *rate > 0 {
		if total != slots {
			panic("fixed schedule lost requests")
		}
		sort.Float64s(scheduled)
		sort.Float64s(dispatch)
		pct := func(a []float64, q float64) float64 { return a[int(float64(len(a)-1)*q)] }
		out["target_rps"], out["scheduled_requests"], out["completed_in_window"] = *rate, slots, onTime
		out["scheduled_p95_us"], out["scheduled_p99_us"] = pct(scheduled, .95), pct(scheduled, .99)
		out["dispatch_p95_us"], out["dispatch_p99_us"] = pct(dispatch, .95), pct(dispatch, .99)
	}
	json.NewEncoder(os.Stdout).Encode(out)
}
