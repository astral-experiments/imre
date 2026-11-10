from source import build
try:
    build().send(None)
except StopIteration as done:
    print(sum(f() for f in done.value.interpolations[0].value))
