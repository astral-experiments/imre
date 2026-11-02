import gc
import io
import os
import tempfile


def current():
    file = io.BytesIO(b"first\nsecond")
    with tempfile._TemporaryFileWrapper(file, "<stream>", delete=False) as wrapper:
        print(wrapper.readline(), wrapper.read(), wrapper.closed)
    print(file.closed)

    with tempfile.NamedTemporaryFile(delete_on_close=False) as named:
        name = named.name
        print(named.write(b"data"), named.seek(0), named.read())
        named.close()
        print(named.closed, os.path.exists(name))
    print(os.path.exists(name))

    named = tempfile.NamedTemporaryFile()
    path, write = named.name, named.write
    del named
    gc.collect()
    print(os.path.exists(path), write(b"x"))
    del write
    gc.collect()
    print(os.path.exists(path))

    class Proxy:
        def __init__(self):
            self.stream = io.BytesIO(b"proxied")

        def __getattr__(self, name):
            return getattr(self.stream, name)

    proxy = Proxy()
    wrapper = tempfile._TemporaryFileWrapper(proxy, "<stream>", delete=False)
    read = wrapper.read
    del wrapper
    gc.collect()
    print(type(proxy.read) is type(len), read())
    del read
    gc.collect()
    print(proxy.closed)
