import api
from states import READY, State


def label(value: api.Mode) -> str:
    match value:
        case api.Mode.ALIAS:
            return value.name
        case State.DONE:
            return value.value


print(api.DEFAULT is State.ACTIVE)
print(READY)
print(label(api.DEFAULT))
print(label(api.Mode.DONE))
