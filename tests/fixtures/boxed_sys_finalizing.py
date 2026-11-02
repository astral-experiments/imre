import atexit
import sys
from tempfile import NamedTemporaryFile
from sys import is_finalizing


class ObservedFile:
    def __init__(self, file, stage):
        self.file = file
        self.stage = stage

    def close(self, predicate=is_finalizing, output=print):
        self.file.close()
        output(self.stage, predicate(), flush=True)


def temp_file(stage):
    tmp = NamedTemporaryFile(dir=".")
    tmp._closer.file = ObservedFile(tmp.file, stage)
    return tmp


def current():
    global kept
    assert sys.is_finalizing is is_finalizing and is_finalizing() is False
    for args, kwargs in (((0,), {}), ((), {"arg": 1})):
        try:
            is_finalizing(*args, **kwargs)
        except TypeError as error:
            print(error)
        else:
            raise AssertionError("is_finalizing accepted arguments")
    temp_file("ordinary").close()
    atexit.register(temp_file("atexit").close)
    # The closer survives exit callbacks and runs during real interpreter shutdown.
    kept = temp_file("shutdown")
