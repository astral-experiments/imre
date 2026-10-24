def main() -> None:
    keys = ["x" * i + "\x00é" for i in range(128)]
    keys.append(keys[-1])
    alias = keys
    # Misses exercise repeated searches. Look up independently built equal
    # strings, a duplicate, and both ends of the ordered sequence.
    for key in ["missing", "missing", "雪", "\x00é", "x" * 65 + "\x00é", keys[-1]]:
        print(key in keys)

    alias[5] = "雪"
    print("雪" in keys)
    print("x" * 5 + "\x00é" in keys)
    alias.sort()
    for key in ["missing", "missing", "雪"]:
        print(key in keys)
    alias.append("!")
    print("!" in keys)
    print(alias.pop() in keys)
    alias.sort(reverse=True)
    for key in ["missing", "missing", "\x00é", "雪"]:
        print(key in keys)


main()
