import os
import tempfile
from urllib import request


def current():
    fd, name = tempfile.mkstemp()
    os.close(fd)
    tracked = request._url_tempfiles
    previous = request._opener
    try:
        # The missing file must not prevent the remaining download from being removed.
        tracked.extend((name + ".missing", name))
        request.install_opener(object())
        request.urlcleanup()
        assert request._opener is None
        assert not os.path.exists(name)
        assert request._url_tempfiles is tracked and not tracked
        request.urlcleanup()
    finally:
        request.install_opener(previous)
        if os.path.exists(name):
            os.unlink(name)
