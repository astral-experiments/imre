import { readFileSync } from "node:fs";
import TinyQueue from "tinyqueue";

const [path, ...queries] = process.argv.slice(2);
let graph;
for (const line of readFileSync(path, "utf8").split("\n")) {
  if (line.startsWith("p ")) {
    graph = Array.from({ length: Number(line.split(" ")[2]) + 1 }, () => []);
  } else if (line.startsWith("a ")) {
    const [, from, to, weight] = line.split(" ");
    graph[Number(from)].push([Number(to), Number(weight)]);
  }
}

function distance(start, end) {
  const best = new Float64Array(graph.length).fill(Infinity);
  best[start] = 0;
  const queue = new TinyQueue([[0, start]], (a, b) => a[0] - b[0]);
  while (queue.length) {
    const [cost, node] = queue.pop();
    if (node === end) return cost;
    if (best[node] !== cost) continue;
    for (const [neighbor, weight] of graph[node]) {
      const next = cost + weight;
      if (next < best[neighbor]) {
        best[neighbor] = next;
        queue.push([next, neighbor]);
      }
    }
  }
  return -1;
}

if (!queries.length)
  console.log(
    graph.length - 1,
    graph.reduce((n, edges) => n + edges.length, 0),
  );
for (const query of queries)
  console.log(distance(...query.split(",").map(Number)));
