from email.message import Message
from http.client import HTTPMessage


def current():
    assert issubclass(HTTPMessage, Message)
    for cls in (Message, HTTPMessage):
        message = cls()
        message["X-Trace"] = "one"
        message["x-trace"] = "two"
        print(message.items(), message["X-TRACE"], message.get_all("x-Trace"))
        print(list(message), len(message), "x-trace" in message, "missing" in message)
        print(message.get("missing", "fallback"), message.get_all("missing", ["fallback"]))
        try:
            message["bad:name"] = "value"
        except ValueError as err:
            print(str(err))
