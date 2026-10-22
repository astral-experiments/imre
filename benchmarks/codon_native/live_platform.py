"""Native C ABI boundary; links only the benchmark Rust platform library."""

import json


class CalledProcessError(Exception):
    def __init__(self, message: str):
        super().__init__(message)


class TimeoutExpired(Exception):
    def __init__(self, message: str):
        super().__init__(message)


def call(operation: int, request: str) -> str:
    if "\0" in request:
        raise ValueError("embedded null byte in native platform request")
    from C import relar_benchmark_platform_call(i32, cobj) -> cobj
    from C import relar_benchmark_platform_free(cobj)
    result = relar_benchmark_platform_call(i32(operation), request.c_str())
    reply_text = str.from_ptr(result)
    relar_benchmark_platform_free(result)
    reply = json.loads(reply_text)
    kind = reply["kind"].as_str()
    message = reply["message"].as_str()
    if kind == "ValueError":
        raise ValueError(message)
    if kind == "UnicodeDecodeError":
        raise UnicodeDecodeError("utf-8", b"", 0, 1, message)
    if kind == "CalledProcessError":
        raise CalledProcessError(message)
    if kind == "TimeoutExpired":
        raise TimeoutExpired(message)
    if kind == "FileNotFoundError":
        raise OSError(2, message)
    if kind == "PermissionError":
        raise OSError(13, message)
    if kind == "FileExistsError":
        raise OSError(17, message)
    if kind == "IsADirectoryError":
        raise OSError(21, message)
    if kind == "NotADirectoryError":
        raise OSError(20, message)
    if kind:
        raise OSError(message)
    return reply["data"].as_str()
