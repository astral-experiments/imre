    terminator: str

    def __init__(self, stream: _TextOutput | None = None) -> None:
        super().__init__()
        self.stream = sys.stderr if stream is None else stream
        self.terminator = "\n"
