import sys

import model
from model import Row as Entry


def classify(value: model.Row | None) -> str:
    match value:
        case Entry(name="ready", count=count):
            return f"ready:{count}"
        case model.Row(name=name):
            return name
        case None:
            return "none"
        case _:
            return "wrong class"


print(classify(Entry("ready", 7)))
print(classify(model.Row("waiting", 3)))
print(classify(None))
match ["arguments"]:
    case ["arguments"] if len(sys.argv) == 1:
        print("argv in guard")
    case _:
        print("wrong arguments")
