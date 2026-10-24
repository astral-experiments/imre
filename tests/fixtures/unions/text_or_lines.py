from dataclasses import dataclass


@dataclass
class Detail:
    content: str | list[str]


def show(content: str | list[str]) -> str:
    if isinstance(content, str):
        return content
    content.append("last")
    return "\n".join(content)


def maybe(value: list[str] | None) -> str | list[str] | None:
    return value


def check(value: str | list[str] | None) -> None:
    print(bool(value))
    print(isinstance(value, list))
    print(isinstance(value, bytes))
    print(repr(value))


lines = ["one", "β"]
detail = Detail(lines)
print(show(detail.content))
print(repr(lines))
print(Detail(["one", "β", "last"]) == detail)
print(Detail("text") == detail)
print(show("unchanged"))
check(maybe(lines))
check(maybe(None))
check([])
print(show([]))
check([*[], "β"])
check("")
