import logging
import sys


def receiver() -> logging.Logger:
    print("receiver")
    return logging.getLogger("child")


def message() -> str:
    print("message")
    return "from child"


def level() -> int:
    print("level")
    return logging.WARNING


logging.basicConfig(stream=sys.stdout, level=logging.INFO, format="%(name)s %(message)s")
logging.info(msg="from root")
receiver().log(msg=message(), level=level())
