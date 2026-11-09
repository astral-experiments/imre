import gc
import sys
import warnings

sys.path


async def job():
    return 1


def forget():
    job()


def capture_job():
    pending = job()
    return sys._getframe()


def raise_with_job():
    pending = job()
    raise LookupError


def capture_traceback():
    try:
        raise_with_job()
    except LookupError as error:
        return error.__traceback__


with warnings.catch_warnings(record=True) as seen:
    warnings.simplefilter("always", RuntimeWarning)
    forget()
    assert len(seen) == 1
    assert seen[0].category is RuntimeWarning
    assert str(seen[0].message) == "coroutine 'job' was never awaited"

    pending = job()

    def get_pending():
        return pending

    get_pending()
    assert len(seen) == 1
    pending.close()


gc.disable()
with warnings.catch_warnings(record=True) as seen:
    warnings.simplefilter("always", RuntimeWarning)
    frame = capture_job()
    assert not seen
    frame.clear()
    assert len(seen) == 1 and seen[0].category is RuntimeWarning

    seen.clear()
    try:
        raise_with_job()
    except LookupError as error:
        assert not seen
        error.with_traceback(None)
        assert len(seen) == 1 and seen[0].category is RuntimeWarning

    seen.clear()
    LookupError().with_traceback(capture_traceback())
    assert len(seen) == 1 and seen[0].category is RuntimeWarning
gc.enable()
