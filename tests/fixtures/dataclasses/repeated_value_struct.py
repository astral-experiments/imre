from copy import deepcopy
from imre import struct
from imre.json import encode


@struct
class Point:
    x: float
    y: float

    def norm2(self) -> float:
        return self.x * self.x + self.y * self.y


def total(points: list[Point]) -> float:
    result = 0.0
    for point in points:
        result += point.norm2()
    return result


def key(point: Point) -> int:
    return int(point.y)


def take(points: list[Point]) -> Point:
    for point in points:
        points.clear()
        return point
    raise ValueError("empty")


def main() -> None:
    seed = Point(-0.0, 4.0)
    points = [seed] * 7
    alias = points
    saved = points[-2]
    sliced = points[::-2]
    clone = deepcopy(points)
    repeated = points * 3
    dense = [seed for i in range(7)]
    print(points == dense)
    print(dense == points)
    print(points == clone)
    print(points == sliced)
    print(seed in repeated)
    print(Point(1.0, 4.0) not in repeated)
    print(total(repeated))
    print(encode(sliced))
    points.sort(key=key)
    points[-1] = seed
    points.append(seed)
    print(points.pop())
    alias[-2] = Point(0.0, 4.0)
    print(points[-2].x)
    print(points[-1].x)
    print(saved.x)
    print(points == clone)
    points[0] = Point(3.0, 4.0)
    print(total(points))
    print(total(clone))
    sliced.append(Point(6.0, 8.0))
    print(total(sliced))
    print(total(repeated))
    print(take(clone))
    print(len(clone))
    single = [seed] * 1
    print(single.pop())
    single.append(Point(5.0, 1.0))
    print(single[0])
    empty = repeated * -4
    print(total(empty))
    missing = repeated[1:1]
    missing.append(Point(2.0, 2.0))
    print(total(missing))
    try:
        repeated[99] = Point(1.0, 2.0)
    except IndexError:
        print(len(repeated))
    for point in repeated:
        repeated[-1] = Point(12.0, 5.0)
        print(point.x)
        break
    print(total(repeated))


main()
