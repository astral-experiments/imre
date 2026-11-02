import json
from json.encoder import JSONEncoder


def option(name: str, enabled: bool) -> bool:
    print(name)
    return enabled


def receiver(encoder: JSONEncoder) -> JSONEncoder:
    print("receiver")
    return encoder


def mapping() -> dict[str, str]:
    print("argument")
    return {"z": "é", "a": "λ"}


def fail_option() -> bool:
    print("failing constructor argument")
    raise ValueError("stop")


encoder = json.JSONEncoder(
    sort_keys=option("sort", True),
    ensure_ascii=option("ascii", False),
    allow_nan=False,
)
alias = encoder
print(alias is encoder)
print(JSONEncoder() is not encoder)
print(receiver(alias).encode(o=mapping()))
print(JSONEncoder().encode(["é", "λ"]))
try:
    encoder.encode(float("nan"))
except ValueError as exc:
    print(str(exc))
print(encoder.encode({}))


try:
    JSONEncoder(ensure_ascii=fail_option()).encode(mapping())
except ValueError as exc:
    print(str(exc))
