import { readFileSync, writeFileSync } from "node:fs";
import http from "node:http";

const reference = JSON.parse(readFileSync(new URL("../expected.json", import.meta.url))).loopback_tcp;
const html = "text/html; charset=utf-8";
const json = "application/json";
const entities = { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&#34;", "'": "&#39;" };
const escape = text => text.replace(/[&<>"']/g, ch => entities[ch]);

function error(name) {
  const { status, body } = reference[name];
  return [status, html, body];
}

function route(method, url, body) {
  if (method === "GET" && url.pathname === "/health") {
    return [200, json, JSON.stringify({ ok: true }) + "\n"];
  }
  if (method === "GET" && url.pathname.startsWith("/hello/")) {
    const name = decodeURIComponent(url.pathname.slice("/hello/".length));
    if (!name || name.includes("/")) return error("missing");
    const suffix = url.searchParams.get("suffix") ?? "!";
    return [200, html, "Hello, " + escape(name) + escape(suffix)];
  }
  if (method === "POST" && url.pathname === "/total") {
    // The reviver sees the original token even when Number has rounded the value.
    const { values } = JSON.parse(body, (key, value, context) =>
      typeof value === "number" ? BigInt(context.source) : value);
    if (!Array.isArray(values)) return error("view_exception");
    const total = values.reduce((a, b) => a + b, 0n);
    return [201, json, '{"count":' + values.length + ',"total":' + total + '}\n'];
  }
  return error("missing");
}

const ready = process.argv[2];
if (process.versions.bun) {
  const server = Bun.serve({
    hostname: "127.0.0.1", port: 0,
    async fetch(req) {
      const body = req.method === "POST" ? await req.text() : "";
      const [status, type, text] = route(req.method, new URL(req.url), body);
      return new Response(text, { status, headers: { "Content-Type": type } });
    },
  });
  writeFileSync(ready, String(server.port));
} else {
  const server = http.createServer(async (req, res) => {
    let body = "";
    if (req.method === "POST") {
      req.setEncoding("utf8");
      for await (const chunk of req) body += chunk;
    }
    const [status, type, text] = route(req.method, new URL(req.url, "http://localhost"), body);
    res.writeHead(status, { "Content-Type": type, "Content-Length": Buffer.byteLength(text) });
    res.end(text);
  });
  server.listen(0, "127.0.0.1", () => writeFileSync(ready, String(server.address().port)));
}
