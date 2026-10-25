"""Compare adapter identity, delegation, and public processing of string contexts."""
import logging


def check() -> None:
    logger = logging.getLogger("app.adapter")
    logger.propagate = False
    defaults = {"shared": "default", "first": "one"}
    event = {"shared": "event", "second": "two"}
    adapter = logging.LoggerAdapter(logger, defaults)
    print(adapter.logger is logger)
    print(adapter.extra is defaults)
    print(adapter.name)
    print(adapter.hasHandlers())
    adapter.setLevel("ERROR")
    print(logger.level)
    print(adapter.getEffectiveLevel())
    print(adapter.isEnabledFor(logging.WARNING))
    logger.setLevel(logging.DEBUG)
    print(adapter.isEnabledFor(logging.INFO))
    kwargs: dict[str, dict[str, str] | None] = {"extra": event, "other": None}
    msg, result = adapter.process("unchanged", kwargs)
    print(msg)
    print(result is kwargs)
    print(result["extra"] is defaults)
    print("other" in result)
    defaults["first"] = "later"
    own = result["extra"]
    if own is not None:
        print(own["first"])
    adapter.merge_extra = True
    kwargs["extra"] = event
    _, result = adapter.process("second", kwargs)
    merged = result["extra"]
    if merged is not None:
        print(list(merged))
        print(merged["shared"])
        print(merged["first"])
        print(merged["second"])
        print(merged is defaults)
        print(merged is event)
        merged["shared"] = "changed only here"
    print(defaults["shared"])
    print(event["shared"])
    empty: dict[str, str] = {}
    kwargs["extra"] = empty
    _, result = adapter.process("empty", kwargs)
    print(result["extra"] is defaults)
    print(result["extra"] is empty)
    kwargs["extra"] = None
    _, result = adapter.process("none", kwargs)
    print(result["extra"] is defaults)
    del kwargs["extra"]
    _, result = adapter.process("absent", kwargs)
    print(result["extra"] is defaults)
    adapter.extra = None
    kwargs["extra"] = event
    _, result = adapter.process("no defaults", kwargs)
    print(result["extra"] is event)
    kwargs["extra"] = None
    _, result = adapter.process("neither", kwargs)
    print(result["extra"] is None)
    other = logging.LoggerAdapter(logger)
    print(other.name)
    print(other.merge_extra)


check()
