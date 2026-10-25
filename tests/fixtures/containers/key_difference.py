def discard_later(mapping: dict[str, int]) -> set[str]:
    mapping["later"] = 1
    return {"old"}


a = {"uv"}
b = {"new"}
print(a == a)
print(a == b)
print(a != b)
print(a != b == {"new"})
print(f"Missing: {a - b}")
print(f"Empty: {a - a}")
quote = {"a'b"}
print(f"Quoted: {quote}")
print(repr(sorted((a | b) - a)))
print(repr(sorted(a)))
mapping = {"old": 2}
print(repr(sorted(mapping.keys() - discard_later(mapping))))
print(repr(sorted({"later", "elsewhere"} - mapping.keys())))

huge = 1 << 120
print({(huge, "a")} == {(huge, "a")})
print(len({(huge, "a")} - {(huge, "b")}))
