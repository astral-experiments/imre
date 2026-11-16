def current():
    import selectors
    import sys

    module_name = "rebound_socketserver"

    class NamespaceKey(str):
        pass

    class Selectors:
        def __getattr__(self, name):
            if name == "PollSelector":
                namespace = sys.modules["socketserver"].__dict__
                del namespace["__name__"]
                namespace[NamespaceKey("__name__")] = module_name
            return getattr(selectors, name)

    sys.modules["selectors"] = Selectors()
    try:
        import socketserver
    finally:
        sys.modules["selectors"] = selectors

    assert socketserver.TCPServer.__module__ is module_name
