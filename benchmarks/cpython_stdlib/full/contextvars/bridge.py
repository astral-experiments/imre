"""Drive public operations against objects retained by a single native process."""
import sys
from contextvars import Context, ContextVar, Token

contexts: list[Context] = []
variables: list[ContextVar[int]] = []
tokens: list[Token[int]] = []


def reply(value: str) -> None:
    print(value)
    sys.stdout.flush()


def maybe(value: int | None) -> str:
    if value is None:
        return "None"
    return str(value)


def operation(action: str) -> None:
    if action == "ctx":
        contexts.append(Context())
        reply(str(len(contexts) - 1))
    elif action == "copy":
        contexts.append(contexts[int(input())].copy())
        reply(str(len(contexts) - 1))
    elif action == "var":
        name = input()
        default = input()
        if default == "missing":
            v: ContextVar[int] = ContextVar(name)
        else:
            v = ContextVar(name, default=int(default))
        variables.append(v)
        reply(str(len(variables) - 1))
    elif action == "set":
        t = variables[int(input())].set(int(input()))
        tokens.append(t)
        reply(str(len(tokens) - 1))
    elif action == "reset":
        variables[int(input())].reset(tokens[int(input())])
        reply("None")
    elif action == "get":
        reply(str(variables[int(input())].get()))
    elif action == "ctx-get":
        reply(maybe(contexts[int(input())].get(variables[int(input())])))
    elif action == "item":
        reply(str(contexts[int(input())][variables[int(input())]]))
    else:
        raise AssertionError("unexpected bridge action")
