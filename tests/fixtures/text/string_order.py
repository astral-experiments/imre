def compare(left: str, right: str) -> None:
    print(left < right)
    print(left <= right)
    print(left > right)
    print(left >= right)


compare("", "")
compare("", "a")
compare("prefix", "prefixes")
compare("prefixes", "prefix")
compare("a\x00b", "a\x00c")
compare("é", "e\u0301")
compare("\uffff", "\U00010000")
compare("🙂", "😀")
compare("同", "同")
print("a" < "é" < "🙂")
