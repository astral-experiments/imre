import re


FIELD = re.compile(r"(?P<prefix>ev|event)?(?P<key>.*?) -> (?P<kind>[A-Z]+) (?P<text>.*)")


def show(match: re.Match[str] | None) -> None:
    if match is None:
        print("miss")
        return
    print(repr(match.groupdict()))
    print(f"{match.start()}-{match.end()}")
    print(f"{match.start('key')}-{match.end('key')} {match.start('text')}-{match.end('text')}")


for line in [
    "event.🌍 -> INFO café\r\nnext",
    "ev -> ERROR ",
    "π -> wrong -> OK done",
    "x\n -> OK restart",
    " -> A \0",
    "miss",
]:
    show(FIELD.match(line))
    show(FIELD.search(line))
