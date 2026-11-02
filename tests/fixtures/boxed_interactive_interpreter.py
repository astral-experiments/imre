import code
import codeop


def current():
    assert code.compile_command is codeop.compile_command
    console = code.InteractiveConsole()
    assert isinstance(console.compile, codeop.CommandCompiler)
    console.buffer.append("unfinished")
    console.resetbuffer()
    assert console.buffer == []

    namespace = {}
    interpreter = code.InteractiveInterpreter(namespace)
    interpreter.runcode(compile("answer = 6 * 7", "<console>", "exec"))
    assert namespace["answer"] == 42
    try:
        interpreter.runcode(compile("raise SystemExit(7)", "<console>", "exec"))
    except SystemExit as error:
        assert error.args == (7,)
    else:
        raise AssertionError("runcode swallowed SystemExit")
