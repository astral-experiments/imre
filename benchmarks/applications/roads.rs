use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::{env, fs};

const END: usize = usize::MAX;

#[derive(Default)]
struct Graph {
    heads: Vec<usize>,
    edges: Vec<Edge>,
}

struct Edge {
    next: usize,
    to: usize,
    weight: u64,
}

impl Graph {
    fn distance(&self, start: usize, end: usize) -> Option<u64> {
        let mut best = vec![u64::MAX; self.heads.len()];
        let mut queue = BinaryHeap::from([Reverse((0, start))]);
        best[start] = 0;
        while let Some(Reverse((cost, node))) = queue.pop() {
            if node == end {
                return Some(cost);
            }
            if cost == best[node] {
                let mut edge = self.heads[node];
                while edge != END {
                    let arc = &self.edges[edge];
                    let neighbor = arc.to;
                    let next = cost + arc.weight;
                    if next < best[neighbor] {
                        best[neighbor] = next;
                        queue.push(Reverse((next, neighbor)));
                    }
                    edge = arc.next;
                }
            }
        }
        None
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut graph = Graph::default();
    for line in fs::read_to_string(args.next().ok_or("missing graph")?)?.lines() {
        let mut fields = line.split_ascii_whitespace();
        match fields.next() {
            Some("p") => {
                let nodes: usize = fields.nth(1).ok_or("missing nodes")?.parse()?;
                graph.heads = vec![END; nodes + 1];
                let arcs: usize = fields.next().ok_or("missing arc count")?.parse()?;
                graph.edges.reserve(arcs);
            }
            Some("a") => {
                let from: usize = fields.next().ok_or("missing start")?.parse()?;
                let to = fields.next().ok_or("missing end")?.parse()?;
                let weight = fields.next().ok_or("missing weight")?.parse()?;
                graph.edges.push(Edge {
                    next: graph.heads[from],
                    to,
                    weight,
                });
                graph.heads[from] = graph.edges.len() - 1;
            }
            _ => {}
        }
    }
    if args.len() == 0 {
        println!("{} {}", graph.heads.len() - 1, graph.edges.len());
    }
    for pair in args {
        let (start, end) = pair.split_once(',').ok_or("invalid query")?;
        if let Some(distance) = graph.distance(start.parse()?, end.parse()?) {
            println!("{distance}");
        } else {
            println!("-1");
        }
    }
    Ok(())
}
