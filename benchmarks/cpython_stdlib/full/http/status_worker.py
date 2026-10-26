"""Serve individual HTTPStatus operations to the two hosted original methods."""
import sys
from http import HTTPStatus


def answer(status: HTTPStatus, operation: str, bound: int) -> bool:
    if operation == "ge":
        return status >= bound
    if operation == "le":
        return status <= bound
    if operation == "is_informational":
        return status.is_informational
    if operation == "is_success":
        return status.is_success
    if operation == "is_redirection":
        return status.is_redirection
    if operation == "is_client_error":
        return status.is_client_error
    if operation == "is_server_error":
        return status.is_server_error
    raise ValueError("unknown status operation")


while True:
    request = input()
    if request == "quit":
        break
    status = HTTPStatus(int(request))
    operation = input()
    bound = int(input())
    print(answer(status, operation, bound))
    sys.stdout.flush()
