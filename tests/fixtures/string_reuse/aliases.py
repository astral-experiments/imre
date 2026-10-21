class Box:
    text: str

    def __init__(self, text: str) -> None:
        self.text = text


def snapshots(text: str) -> str:
    local = text
    listed = [text]
    mapping = {text: "key", "value": text}
    members = {text}
    pair = (text, 7)
    box = Box(text)
    text += " changed"
    print(local)
    print(listed[0])
    print(mapping[local])
    print(mapping["value"])
    print(local in members)
    print(pair[0])
    print(box.text)
    for suffix in ["one", "two", "three"]:
        listed.append(text)
        text = text + suffix
    for previous in listed:
        print(previous)
    box.text = box.text + " field"
    print(local)
    print(box.text)
    return text


def borrowed(text: str, suffix: str) -> str:
    return text + suffix


def rebound(text: str, suffix: str) -> str:
    text += suffix
    text = text + " tail"
    return text


original = "heap-sized original with Unicode é😀 and " + str(123)
print(snapshots(original))
print(original)
print(borrowed(original, original))
print(rebound(original, original))
print(original)
