class Label(str):
    def replace(self, old, new):
        print("replace", old, new)
        return "overridden"


def normalize(label: str) -> str:
    return label.replace("-", "_")


def invoke(label: str) -> str:
    return normalize(label)


first = normalize
print(invoke("one-two"))
print(invoke(Label("one-two")))
globals()["normalize"] = str.upper
print(invoke("one-two"))
print(first("one-two"))
