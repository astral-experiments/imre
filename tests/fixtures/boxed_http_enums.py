"""Exercise the unmodified HTTP initializer loaded as the CLI's source plugin."""
from arithmetic import HTTPMethod, HTTPStatus
from enum import Enum


print(len(HTTPStatus.__members__), len(HTTPStatus), len(HTTPMethod))
for name, member in HTTPStatus.__members__.items():
    print(name, member.name, member.value, member.phrase, member.description)
    print(HTTPStatus[name] is HTTPStatus(member.value) is member)
for method in HTTPMethod:
    print(str(method), repr(method), method.description, HTTPMethod(method.value) is method)

responses = {member: (member.phrase, member.description) for member in HTTPStatus}
print(responses[404], responses[HTTPStatus.REQUEST_ENTITY_TOO_LARGE])
responses[413] = ("updated", "")
print(len(responses), responses[HTTPStatus.CONTENT_TOO_LARGE])
for key in responses:
    print(type(key) is HTTPStatus, HTTPStatus(key.value) is key)
try:
    HTTPStatus.__members__["NOT_FOUND"] = HTTPStatus.OK
except TypeError:
    print(HTTPStatus.NOT_FOUND is HTTPStatus(404))


class COEP(Enum):
    UNSAFE_NONE = "unsafe-none"
    REQUIRE_CORP = "require-corp"

    @staticmethod
    def identity(value):
        return value


class COOP(Enum):
    UNSAFE_NONE = "unsafe-none"
    SAME_ORIGIN_ALLOW_POPUPS = "same-origin-allow-popups"
    SAME_ORIGIN = "same-origin"


for policy in (COEP, COOP):
    for member in policy:
        print(member.name, member.value, str(member), repr(member))
        print(policy(member.value) is member, policy[member.name] is member)
print(tuple(COEP.__members__), COEP.identity("static"))
print(COEP.UNSAFE_NONE is COOP.UNSAFE_NONE, COEP.UNSAFE_NONE == COOP.UNSAFE_NONE)
print(isinstance(COEP.UNSAFE_NONE, str), COEP.UNSAFE_NONE == "unsafe-none")
