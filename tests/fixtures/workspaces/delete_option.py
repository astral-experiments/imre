from pathlib import Path
from tempfile import TemporaryDirectory


with TemporaryDirectory(delete=False) as name:
    assert Path(name).is_dir()
Path(name).rmdir()
