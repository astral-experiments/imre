import logging


def render(pattern: str, value: int) -> None:
    try:
        print(len(pattern % (value,)))
    except ValueError as error:
        print(str(error))


boundary = 10**4300
for pattern in ["%d", "%i", "%u", "%s", "%r", "%a", "%.2s", "%#x", "%o"]:
    for value in [boundary - 1, boundary, -boundary]:
        render(pattern, value)

for pattern in ["%r%q", "%q%r"]:
    try:
        print(pattern % (boundary, 0))
    except ValueError as error:
        print(str(error))

try:
    print(logging.LogRecord("test", 20, "file", 1, "%d", (boundary,), None).getMessage())
except ValueError as error:
    print(str(error))

print("%.8a/%a" % ("éπ💙", "éπ💙"))
