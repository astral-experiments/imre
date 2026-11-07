def collect(words: list[str]) -> list[str]:
    return [word for _ in range(2) for word in iter(lambda: words.pop(), "stop")]


items = ["stop", "local second", "stop", "local first"]
for word in collect(items):
    print(word)
print(len(items))
