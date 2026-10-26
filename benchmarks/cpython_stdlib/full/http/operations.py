"""Direct public API comparison. These probes are outside the CPython suite numerator."""
import http
from enum import IntEnum, StrEnum
from http import HTTPMethod, HTTPStatus


def describe(status: HTTPStatus) -> None:
    print(status.name)
    print(status.value)
    print(status.phrase)
    print(status.description)
    print(int(status))
    print(str(status))
    print(repr(status))
    print(isinstance(status, int))
    print(isinstance(status, IntEnum))
    print(isinstance(status, HTTPStatus))
    print(status.is_informational)
    print(status.is_success)
    print(status.is_redirection)
    print(status.is_client_error)
    print(status.is_server_error)


def by_code(code: int) -> HTTPStatus:
    return http.HTTPStatus(code)


def by_verb(verb: str) -> HTTPMethod:
    return http.HTTPMethod(verb)


def by_name(name: str) -> HTTPStatus:
    return HTTPStatus[name]


def distinct(code: int) -> bool:
    return HTTPStatus(code) is not code


describe(HTTPStatus.OK)
describe(by_code(404))
print(by_code(200) is HTTPStatus.OK)
print(HTTPStatus(HTTPStatus.NOT_FOUND) is HTTPStatus.NOT_FOUND)
print(HTTPStatus.REQUEST_ENTITY_TOO_LARGE is HTTPStatus.CONTENT_TOO_LARGE)
print(HTTPStatus.REQUEST_ENTITY_TOO_LARGE.name)
print(HTTPStatus.NOT_FOUND == 404)
print(200 <= HTTPStatus.OK < HTTPStatus.NOT_FOUND)
print(isinstance(200, HTTPStatus))
for code in [0, 199, 999, 10000000000000000000000000]:
    try:
        by_code(code)
    except ValueError as error:
        print(str(error))
try:
    HTTPStatus("200")
except ValueError as error:
    print(str(error))
for verb in ["GET", "POST", "TRACE"]:
    method = by_verb(verb)
    print(method.name)
    print(method.value)
    print(method.description)
    print(str(method))
    print(repr(method))
    print(isinstance(method, str))
    print(isinstance(method, StrEnum))
    print(method == verb)
    print(method is HTTPMethod.GET)
try:
    by_verb("get")
except ValueError as error:
    print(str(error))


print(distinct(200))
print(HTTPStatus["OK"] is HTTPStatus.OK)
print(by_name("REQUEST_URI_TOO_LONG") is HTTPStatus.URI_TOO_LONG)
try:
    by_name("MISSING")
except KeyError as error:
    print(str(error))

# Compare the declaration order, every payload and every alias against CPython.
for status in HTTPStatus.__members__.values():
    describe(status)
    print(by_code(int(status)) is status)
for status in HTTPStatus:
    print(status.name)
for method in HTTPMethod:
    print(method.name)
    print(method.description)
    print(by_verb(method.value) is method)
