from dataclasses import dataclass, field


@dataclass
class Number:
    value: float


@dataclass
class Diagnostic:
    label: str
    samples: list[float] = field(default_factory=list, compare=False)


@dataclass(eq=False)
class FloatStorage:
    samples: list[float]


first = Number(float("nan"))
second = Number(float("nan"))
print(first == first)
print(first == second)
print(Number(-0.0) == Number(0.0))
print(Number(float("inf")))
print(Diagnostic("sample", [float("nan"), -0.0]) == Diagnostic("sample", [1.0]))
print(Diagnostic("sample", [float("nan"), -0.0]))
values = FloatStorage([float("nan")])
print(values == values)
