def source(text: str) -> str:
    print("source")
    return text


def start() -> int:
    print("start")
    return -2


def fields(text: str, sep: str) -> None:
    original = text
    saved: list[str] = []
    for field in text.split("|"):
        text = "rebound"
        saved.append(field)
    print(saved)

    for i, field in enumerate(source(original).split("|"), start()):
        print(f"{i}:{field}")
        if field == "stop":
            break

    listed = original.split("|")
    listed.append("extra")
    print(listed)
    for field in original.split(sep):
        print(field)
    for field in original.split("|", 1):
        print(field)
    for field in original.split("🌱"):
        print(field)


for value in ["", "plain", "||", "|é|🌱||", "start|stop|later"]:
    fields(value, "|")
