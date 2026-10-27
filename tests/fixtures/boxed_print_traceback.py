import sys
from traceback import print_exception


class Writer:
    def __init__(self):
        self.chunks = []

    def write(self, text):
        self.chunks.append(text)


def missing(data):
    return data['value']


def call():
    return missing({})


def capture(exc, **options):
    stream = Writer()
    print_exception(exc, file=stream, **options)
    print(repr(stream.chunks))


try:
    call()
except KeyError as original:
    tb = original.__traceback__
    print(tb.tb_frame.f_code.co_name, tb.tb_frame.f_code.co_filename == __file__)
    print(tb.tb_next.tb_frame.f_code is call.__code__)
    print(tb.tb_next.tb_next.tb_frame.f_code is missing.__code__)
    capture(original, limit=-1)
    for cause in (original, None):
        try:
            raise RuntimeError('outer') from cause
        except RuntimeError as outer:
            print(outer.__context__ is original, outer.__cause__ is cause, outer.__suppress_context__)
            capture(outer, limit=0)
            capture(outer, limit=0, chain=False)

print_exception(None, file=Writer())
print(sys.exception() is None)
