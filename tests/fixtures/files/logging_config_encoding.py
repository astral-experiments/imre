"""Configure live loggers from encoded INI files and preserve strict read errors."""
import logging
import logging.config
from pathlib import Path


def apply(path: Path, contents: bytes, encoding: str, name: str) -> None:
    path.write_bytes(contents)
    logging.getLogger().setLevel(logging.CRITICAL)
    logger = logging.getLogger(name)
    logger.setLevel(logging.NOTSET)
    try:
        logging.config.fileConfig(str(path), encoding=encoding)
    except UnicodeDecodeError as exc:
        print(str(exc))
    except LookupError as exc:
        print(str(exc))
    except ValueError as exc:
        print(str(exc))
    print(logging.getLogger().level)
    print(logger.level)


config = ("[formatters]\nkeys=\n[handlers]\nkeys=\n[loggers]\nkeys=root,child\n"
          "[logger_root]\nlevel=WARNING\nhandlers=\n"
          "[logger_child]\nlevel=INFO\nhandlers=\nqualname=")
path = Path("encoding.ini")
apply(path, (config + "ascii\n").encode(), "US-ASCII", "ascii")
apply(path, (config + "café\n").replace("\n", "\r").encode("utf-8-sig"), "UTF_8_SIG", "café")
apply(path, (config + "Журнал\n").encode("cp1251"), "windows-1251", "Журнал")
apply(path, b"#" + b"a" * 8191 + b"abc\xff", "ascii", "ascii")
apply(path, b"\xef\xbb\xbf#" + b"a" * 8187 + b"\xe2\x82!", "utf-8-sig", "café")
apply(path, b"#a\xe2\x82", "utf-8", "ascii")
apply(path, b"#", "missing-codec", "ascii")
apply(path, b"#", "utf-8\0", "ascii")
