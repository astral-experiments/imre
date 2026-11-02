"""Find shortest-path distances in a directed DIMACS road graph."""

import sys

from route_planner import heap_pop, heap_push


def load_graph(path: str) -> list[list[tuple[int, int]]]:
    graph: list[list[tuple[int, int]]] = []
    with open(path, encoding="utf-8") as source:
        for line in source:
            if line.startswith("p "):
                nodes = int(line.split()[2])
                for _ in range(nodes + 1):
                    graph.append([])
            elif line.startswith("a "):
                columns = line.split()
                graph[int(columns[1])].append((int(columns[2]), int(columns[3])))
    return graph


def distance(graph: list[list[tuple[int, int]]], start: int, end: int) -> int:
    best = [-1] * len(graph)
    best[start] = 0
    nodes = [start]
    costs = [0]
    while nodes:
        cost = costs[0]
        node = heap_pop(nodes, costs)
        if node == end:
            return cost
        if cost == best[node]:
            for neighbor, weight in graph[node]:
                candidate = cost + weight
                if best[neighbor] == -1 or candidate < best[neighbor]:
                    best[neighbor] = candidate
                    heap_push(nodes, costs, neighbor, candidate)
    return -1


def main() -> None:
    graph = load_graph(sys.argv[1])
    if len(sys.argv) == 2:
        edges = 0
        for neighbors in graph:
            edges += len(neighbors)
        print(f"{len(graph) - 1} {edges}")
    for pair in sys.argv[2:]:
        endpoints = pair.split(",")
        print(distance(graph, int(endpoints[0]), int(endpoints[1])))


main()
