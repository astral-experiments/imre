    # Runtime-loaded streams can expose arbitrary Python name attributes.
    def __repr__(self) -> str:
        level = getLevelName(self.level)
        name = str(getattr(self.stream, "name", ""))
        if name:
            name += " "
        return "<%s %s(%s)>" % (self.__class__.__name__, name, level)
