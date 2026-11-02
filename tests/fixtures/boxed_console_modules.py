def current():
    import code
    import codeop
    from code import InteractiveConsole

    assert code.compile_command is codeop.compile_command
    console = InteractiveConsole()
    assert isinstance(console, code.InteractiveInterpreter)
    assert isinstance(console.compile, codeop.CommandCompiler)
    assert isinstance(console.compile.compiler, codeop.Compile)

    console.runcode(compile("answer = 6 * 7", "<console>", "exec"))
    assert console.locals["answer"] == 42
    console.resetbuffer()
    assert console.buffer == []
