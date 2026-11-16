OFFSET = 1024


def masked(OFFSET: int) -> int:
    OFFSET = OFFSET >> 1
    return OFFSET ^ 300


print(masked(24), OFFSET)
