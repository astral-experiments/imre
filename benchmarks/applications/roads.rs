use std::cmp::Reverse;
use std::collections::BinaryHeap;
use std::{env, fs};

fn distance(graph: &[Vec<(usize, u64)>], start: usize, end: usize) -> Option<u64> {
    let mut best = vec![u64::MAX; graph.len()];
    best[start] = 0;
    let mut queue = BinaryHeap::from([Reverse((0, start))]);
    while let Some(Reverse((cost, node))) = queue.pop() {
        if node == end {
            return Some(cost);
        }
        if best[node] == cost {
            for &(neighbor, weight) in &graph[node] {
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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let mut graph = Vec::new();
    for line in fs::read_to_string(args.next().ok_or("missing graph")?)?.lines() {
        let mut fields = line.split_ascii_whitespace();
        match fields.next() {
            Some("p") => {
                let nodes: usize = fields.nth(1).ok_or("missing nodes")?.parse()?;
                graph.resize_with(nodes + 1, Vec::new);
            }
            Some("a") => {
                let from: usize = fields.next().ok_or("missing start")?.parse()?;
                let to = fields.next().ok_or("missing end")?.parse()?;
                let weight = fields.next().ok_or("missing weight")?.parse()?;
                graph[from].push((to, weight));
            }
            _ => {}
        }
    }
    if args.len() == 0 {
        println!(
            "{} {}",
            graph.len() - 1,
            graph.iter().map(Vec::len).sum::<usize>()
        );
    }
    for pair in args {
        let (from, to) = pair.split_once(',').ok_or("invalid query")?;
        match distance(&graph, from.parse()?, to.parse()?) {
            Some(cost) => println!("{cost}"),
            None => println!("-1"),
        }
    }
    Ok(())
}
