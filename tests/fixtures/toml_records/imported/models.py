from dataclasses import dataclass


@dataclass(slots=True)
class Group:
    name: str
    href: str
