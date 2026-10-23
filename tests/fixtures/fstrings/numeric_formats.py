def later() -> int:
    print("must not evaluate later")
    return 1


def counted(value: int) -> int:
    print(f"field {value}")
    return value


for number in [-1267650600228229401496703205376, -1, 0, 1, 1267650600228229401496703205376]:
    print(f"{counted(number):+}|{number:+d}|{number:d}|{number:-d}|{number: d}")
print(f"{True:+}|{False: }|{True:-}|{False:d}")
for value in [0.0, -0.0, 2.5, 3.5, -2.5, -3.5, 2.675, 1.005, 0.125, 1e-300, 1.7e308, float("inf"), float("-inf"), float("nan")]:
    print(f"{value:.0f}|{value:.2f}|{value:f}|{value:.2%}|{value:%}")
    print(f"{value:+.2%}|{value: .0f}|{value:-.2%}|{value:+f}")
for number in [0, -1, 9007199254740993, 1267650600228229401496703205376]:
    print(f"{number:.0f}|{number:.2%}")
print(f"{True:.2f}|{False:.0%}")
print(len(f"{1.0:.1000f}"))

digits = "1"
for index in range(400):
    digits += "0"
huge = int(digits)
try:
    print(f"{huge:.2f}|{later()}")
except OverflowError:
    print("format overflow")
