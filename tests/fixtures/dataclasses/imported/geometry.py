import imre


@imre.struct
class Point:
    x: float
    y: float

    def scaled_x(self, factor: float) -> float:
        return self.x * factor


def make(x: float) -> list[Point]:
    return [Point(x, 2.0)]
