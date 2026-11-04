    terminator = "\n"

    def __init__(self, stream: _TextOutput | None = None) -> None:
        super().__init__()
        self.stream = sys.stderr if stream is None else stream

    # Runtime-loaded streams can expose arbitrary Python name attributes.
    def __repr__(self) -> str:
        level = getLevelName(self.level)
        name = getattr(self.stream, "name", "")
        name = str(name)
        if name:
            name += " "
        return "<%s %s(%s)>" % (self.__class__.__name__, name, level)
