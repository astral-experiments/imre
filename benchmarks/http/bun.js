Bun.serve({hostname: "127.0.0.1", port: Number(process.argv[2]), idleTimeout: 120,
  fetch(req) {
    const ok = req.method === "GET" && new URL(req.url).pathname === "/hello";
    const body = ok ? "hello\n" : "not found\n";
    return new Response(body, {status: ok ? 200 : 404, headers: {"Content-Type": "text/plain", "Content-Length": String(body.length)}});
  }});
