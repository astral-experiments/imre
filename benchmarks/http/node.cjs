const http = require("node:http");
const server = http.createServer((req, res) => {
  const ok = req.method === "GET" && req.url === "/hello";
  const body = ok ? "hello\n" : "not found\n";
  res.sendDate = false;
  res.writeHead(ok ? 200 : 404, {"Content-Type": "text/plain", "Content-Length": Buffer.byteLength(body)});
  res.end(body);
});
server.keepAliveTimeout = 120000;
server.listen({port: Number(process.argv[2]), host: "127.0.0.1", backlog: 128});
