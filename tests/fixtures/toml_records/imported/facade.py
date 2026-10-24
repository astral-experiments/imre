import imre.toml as wire
from models import Group


def parse(text: str) -> Group:
    return wire.decode(text, Group)
