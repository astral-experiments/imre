def selected(text: str, keys: set[str]) -> set[str]:
    return {ch for ch in text if ch in keys}


def selected_or_control(text: str, keys: set[str]) -> set[str]:
    return {ch for ch in text if ch in keys or ord(ch) < 32}


keys = {"a", "α", "🙂", "ab", ""}
text = "🙂aαα\x1fx🙂"
print(sorted(selected(text, keys)))
print(sorted(selected_or_control(text, keys)))
result = selected("", keys)
result.add("x")
print(sorted(result))
print(sorted(selected("", keys)))
keys.discard("α")
keys.add("x")
print(sorted(selected(text, keys)))

# Exercise a large table, including independent Unicode keys and long strings.
for index in range(64):
    keys.add(str(index))
print(sorted(selected_or_control(text, keys)))
