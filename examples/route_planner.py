"""Plan least-cost routes across generated terrain using a first-party binary heap.

Input: grid width, number of routes, and terrain seed (one integer per line).
Output per route: minimum travel cost, path length, and number of settled cells.
Moving into a cell costs its terrain weight; the starting cell is free.
"""


def heap_push(nodes: list[int], costs: list[int], node: int, cost: int) -> None:
    nodes.append(node)
    costs.append(cost)
    position = len(nodes) - 1
    while position > 0:
        parent = (position - 1) // 2
        if costs[parent] <= cost:
            break
        nodes[position] = nodes[parent]
        costs[position] = costs[parent]
        position = parent
    nodes[position] = node
    costs[position] = cost


def heap_pop(nodes: list[int], costs: list[int]) -> int:
    result = nodes[0]
    last_node = nodes.pop()
    last_cost = costs.pop()
    length = len(nodes)
    if length > 0:
        position = 0
        while position * 2 + 1 < length:
            child = position * 2 + 1
            if child + 1 < length and costs[child + 1] < costs[child]:
                child += 1
            if costs[child] >= last_cost:
                break
            nodes[position] = nodes[child]
            costs[position] = costs[child]
            position = child
        nodes[position] = last_node
        costs[position] = last_cost
    return result


def terrain(width: int, seed: int) -> list[int]:
    weights: list[int] = []
    state = seed
    for cell in range(width * width):
        state = (state * 1664525 + 1013904223) % 4294967296
        weights.append(state % 9 + 1)
    return weights


def relax(
    weights: list[int],
    distances: list[int],
    parents: list[int],
    nodes: list[int],
    costs: list[int],
    origin: int,
    neighbor: int,
    cost: int,
) -> None:
    candidate = cost + weights[neighbor]
    if candidate < distances[neighbor]:
        distances[neighbor] = candidate
        parents[neighbor] = origin
        heap_push(nodes, costs, neighbor, candidate)


def shortest_path(weights: list[int], width: int, statistics: list[int]) -> list[int]:
    count = width * width
    distances = [count * 10] * count
    parents = [-1] * count
    nodes: list[int] = []
    costs: list[int] = []
    distances[0] = 0
    heap_push(nodes, costs, 0, 0)
    settled = 0
    while len(nodes) > 0:
        cost = costs[0]
        node = heap_pop(nodes, costs)
        if cost != distances[node]:
            continue
        settled += 1
        if node == count - 1:
            path: list[int] = []
            cursor = node
            while cursor != -1:
                path.append(cursor)
                cursor = parents[cursor]
            statistics.append(cost)
            statistics.append(settled)
            return path
        row = node // width
        column = node % width
        if row > 0:
            relax(weights, distances, parents, nodes, costs, node, node - width, cost)
        if row + 1 < width:
            relax(weights, distances, parents, nodes, costs, node, node + width, cost)
        if column > 0:
            relax(weights, distances, parents, nodes, costs, node, node - 1, cost)
        if column + 1 < width:
            relax(weights, distances, parents, nodes, costs, node, node + 1, cost)
    assert False
    return []


def validate_path(weights: list[int], width: int, path: list[int], cost: int) -> None:
    assert path[0] == width * width - 1
    assert path[-1] == 0
    actual_cost = 0
    for position in range(len(path) - 1):
        cell = path[position]
        previous = path[position + 1]
        difference = cell - previous
        assert (
            difference == width
            or difference == -width
            or ((difference == 1 or difference == -1) and cell // width == previous // width)
        )
        actual_cost += weights[cell]
    assert actual_cost == cost


def main() -> None:
    width = int(input())
    routes = int(input())
    seed = int(input())
    assert width > 0
    assert routes > 0
    for route in range(routes):
        weights = terrain(width, seed + route)
        statistics: list[int] = []
        path = shortest_path(weights, width, statistics)
        validate_path(weights, width, path, statistics[0])
        print(statistics[0])
        print(len(path) - 1)
        print(statistics[1])


main()
