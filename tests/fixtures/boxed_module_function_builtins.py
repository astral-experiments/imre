import json
import types


def current():
    values = json.loads("[]")
    token = object()
    captured = {"token": token}
    namespace = {"values": values, "__builtins__": captured}
    code = compile("values.append(token)", "<module function>", "exec")
    function = types.FunctionType(code, namespace)

    namespace["__builtins__"] = {}
    assert function() is None
    del namespace["__builtins__"]
    assert function() is None
    assert function.__builtins__ is captured
    assert "__builtins__" not in namespace
    assert len(values) == 2 and values[0] is token and values[1] is token
