def record(group: str = "default", /, **fields: list[int]) -> str:
    for name in fields:
        group += " " + name
        fields[name].append(2)
    fields.clear()
    return group


def sample(name: str) -> None:
    shared = [1]
    fields = {"first": shared}
    fields[name] = shared
    print(record(**fields))
    print(record("second", **fields))
    print(len(fields), len(shared), shared[-1])


sample("group")
