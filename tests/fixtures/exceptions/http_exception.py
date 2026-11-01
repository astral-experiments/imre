from http.client import HTTPException


class ResponseError(HTTPException):
    pass


def request(failure: int) -> None:
    if failure == 0:
        raise HTTPException("invalid response")
    if failure == 1:
        raise ResponseError("incomplete response")
    if failure == 2:
        raise OSError("connection closed")
    raise ValueError("unrelated error")


for failure in range(4):
    try:
        request(failure)
    except (OSError, HTTPException) as error:
        print(str(error))
    except ValueError:
        print("unrelated")
