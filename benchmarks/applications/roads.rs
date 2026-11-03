use smallvec::SmallVec;
use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::{env, fs};

const WIDE: u64 = 1 << 63;
const WEIGHT_BITS: u32 = 31;
const WEIGHT_MASK: u64 = (1 << WEIGHT_BITS) - 1;

#[derive(Default)]
struct Graph {
    rows: Vec<SmallVec<[u64; 4]>>,
    wide: Vec<(usize, u64)>,
}

impl Graph {
    fn add(&mut self, from: usize, to: usize, weight: u64) {
        // The high bit marks an index into the table of edges that don't fit.
        let edge = if let Ok(to) = u32::try_from(to)
            && weight <= WEIGHT_MASK
        {
            (u64::from(to) << WEIGHT_BITS) | weight
        } else {
            let index = self.wide.len();
            self.wide.push((to, weight));
            WIDE | index as u64
        };
        self.rows[from].push(edge);
    }

    fn distance(&self, start: usize, end: usize) -> Option<u64> {
        let mut best = vec![u64::MAX; self.rows.len()];
        let mut queue = BinaryHeap::from([Reverse((0, start))]);
        best[start] = 0;
        while let Some(Reverse((cost, node))) = queue.pop() {
            if node == end {
                return Some(cost);
            }
            if cost == best[node] {
                for &edge in &self.rows[node] {
                    let (neighbor, weight) = if edge & WIDE == 0 {
                        ((edge >> WEIGHT_BITS) as usize, edge & WEIGHT_MASK)
                    } else {
                        self.wide[(edge & !WIDE) as usize]
                    };
                    let next = cost + weight;
                    if next < best[neighbor] {
                        best[neighbor] = next;
                        queue.push(Reverse((next, neighbor)));
                    }
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
                let _: usize = fields.next().ok_or("missing arc count")?.parse()?;
                graph.rows.resize_with(nodes + 1, SmallVec::new);
            }
            Some("a") => {
                let from = fields.next().ok_or("missing start")?.parse()?;
                let to = fields.next().ok_or("missing end")?.parse()?;
                let weight = fields.next().ok_or("missing weight")?.parse()?;
                graph.add(from, to, weight);
            }
            _ => {}
        }
    }
    if args.len() == 0 {
        let edges: usize = graph.rows.iter().map(SmallVec::len).sum();
        println!("{} {}", graph.rows.len() - 1, edges);
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
