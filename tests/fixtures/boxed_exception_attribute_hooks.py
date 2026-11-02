class Error(Exception):
    def __getattribute__(self, name):
        if name == "value":
            return "override"
        if name == "broken":
            raise ValueError("broken")
        return object.__getattribute__(self, name)

    def __getattr__(self, name):
        return name.upper()


def current():
    error = Error("message")
    print(error.value, error.missing, error.args)
    try:
        print(error.broken)
    except ValueError as exc:
        print(type(exc).__name__, str(exc))
