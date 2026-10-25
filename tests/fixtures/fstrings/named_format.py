from typing import Literal


def location(template: Literal["#{line}"]) -> str:
    return template.format(line=7)


def receiver(events: list[int]) -> str:
    events.append(0)
    return "{{{label!s}}}: {line:}/{line}/🦀"


def number(events: list[int], value: int) -> int:
    events.append(value)
    return value


def check(template: str) -> None:
    try:
        print(template.format(label="value"))
    except (KeyError, ValueError, IndexError) as exc:
        print(str(exc))


def main() -> None:
    events: list[int] = []
    print(location("#{line}"))
    print(
        receiver(events).format(
            unused=number(events, 1), line=number(events, 2), label="{line}"
        )
    )
    print(repr(events))
    print("missing fields: {{none}}".format())
    print(
        "{name} {big}".format(name="naïve\0text", big=1234567890123456789012345678901)
    )
    for template in [
        "{{}}",
        "{label}{label}",
        "{missing}}",
        "a{",
        "}",
        "{label",
        "{a{b}",
        "{}",
        "{٠٢}",
        "{9223372036854775808}",
        "{9223372036854775807x}",
        "{9223372036854775808x}",
    ]:
        check(template)
    for template in [
        "{missing!}",
        "{missing!ss}",
        "{label!",
        "{label!s",
        "{label:",
        "{missing!s:}",
        "{missing}{label!}",
    ]:
        check(template)
    try:
        print("{missing}".format(ignored=number(events, 3) // 0))
    except ZeroDivisionError:
        print(repr(events))


main()
