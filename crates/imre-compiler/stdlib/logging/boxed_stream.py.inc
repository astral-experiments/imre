    # Runtime-loaded streams can expose arbitrary Python name attributes.
    def __repr__(self) -> str:
        level = getLevelName(self.level)
        name = getattr(self.stream, "name", "")
        name = str(name)
        if name:
            name += " "
        return "<%s %s(%s)>" % (self.__class__.__name__, name, level)
