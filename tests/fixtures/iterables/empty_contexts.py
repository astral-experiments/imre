def consume(words: list[str]) -> int:
    return len(words)


default = list()
default.append(4)
print(default[0])
words: list[str] = list()
words.append("word")
print(words[0])
empty_tuple: list[str] = list(())
print(len(empty_tuple))
print(consume(list()))
print(consume(list([])))
rows: list[list[str]] = list([[]])
rows[0].append("nested")
print(rows[0][0])
flag = True
selected: list[str] = list() if flag else list(["unused"])
print(len(selected))
fallback: list[str] = list() or list(["fallback"])
print(fallback[0])
kept: list[str] = list(["kept"]) and list()
print(len(kept))
