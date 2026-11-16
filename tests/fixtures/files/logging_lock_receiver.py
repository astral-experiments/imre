import logging


def clear(handler: logging.Handler) -> bool:
    print("clearing lock")
    handler.lock = None
    return False


def acquire(handler: logging.Handler) -> None:
    try:
        acquired = handler.lock.acquire(clear(handler))
    except AttributeError as error:
        print(str(error))
    else:
        print(acquired, handler.lock is None)


acquire(logging.NullHandler())
acquire(logging.Handler())
