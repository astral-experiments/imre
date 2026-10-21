from models import ExportedCounter as Count


def passthrough(value: Count) -> Count:
    return value
