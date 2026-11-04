ASCII_KEYS = {"a", "#", "\x7f"}
UNICODE_KEYS = {"a", "é", "🙂"}
ALIASED_KEYS = {"a"}


def selected(text: str, keys: set[str]) -> set[str]:
    return {ch for ch in text if ch in keys}


def selected_or_control(text: str, keys: set[str]) -> set[str]:
    return {ch for ch in text if ch in keys or ord(ch) < 32}


def ascii_count(text: str) -> int:
    found = {ch for ch in text if ch in ASCII_KEYS or ord(ch) < 32}
    if found:
        return len(found)
    return 0


def unicode_count(text: str) -> int:
    found = {ch for ch in text if ch in UNICODE_KEYS}
    if found:
        return len(found)
    return 0


def aliased_count(text: str) -> int:
    found = {ch for ch in text if ch in ALIASED_KEYS}
    if found:
        return len(found)
    return 0


def wide_count(text: str) -> int:
    found = {ch for ch in text if ch in ASCII_KEYS or ord(ch) < 256}
    if found:
        return len(found)
    return 0


def materialized(text: str) -> set[str]:
    return {ch for ch in text if ch in ASCII_KEYS}


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

# Replace an ASCII key without changing the set size between scans.
keys.discard("a")
keys.add("b")
print(sorted(selected("abx", keys)))

# Exercise a large table, including independent Unicode keys and long strings.
for index in range(64):
    keys.add(str(index))
print(sorted(selected_or_control(text, keys)))

for value in ("", "ordinary", "é🙂", "\x00\x1f\x20\x7f", "🙂#aaé"):
    print(ascii_count(value))
    print(unicode_count(value))
    print(wide_count(value))
    print(aliased_count(value))
alias = ALIASED_KEYS
alias.add("🙂")
print(aliased_count("🙂a"))
first = materialized("🙂")
first.add("fresh")
print(sorted(first))
print(sorted(materialized("🙂")))
