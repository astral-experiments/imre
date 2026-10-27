import heapq


def current():
    values = [4, 1, 8, 7]
    print(heapq.nlargest(2, iter(values)))
    print(heapq.nsmallest(2, values))
    print(heapq.nlargest(2, ["cat", "horse", "cow"], key=len))
    print(values)

    heapq.heapify(values)
    heapq.heappush(values, 3)
    print(heapq.heapreplace(values, 5))
    print([heapq.heappop(values) for _ in range(len(values))])

    heapq.heapify_max(values)
    heapq.heappush_max(values, 9)
    print(heapq.heappushpop_max(values, 3), heapq.heappop_max(values))
