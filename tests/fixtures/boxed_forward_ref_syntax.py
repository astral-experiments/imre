from annotationlib import ForwardRef
from typing import _make_forward_ref


def current():
    for newline in ("\n", "\r", "\r\n"):
        for line in ("('é', (x := ))", "('é', {**d for i in xs})"):
            source = newline.join(("(", line, ")"))
            try:
                compile(source, "<string>", "eval")
            except SyntaxError as error:
                expected = error.args
            else:
                raise AssertionError("invalid annotation compiled")
            for make in (_make_forward_ref, lambda s: ForwardRef(s).evaluate()):
                try:
                    make(source)
                except SyntaxError as error:
                    assert error.args == (f"Forward reference must be an expression -- got {source!r}",)
                    assert error.__context__.args == expected
                else:
                    raise AssertionError("invalid forward reference accepted")
