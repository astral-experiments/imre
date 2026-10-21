def measure(value: str) -> int:
    return len(value) + (saved := len(value)) + saved
