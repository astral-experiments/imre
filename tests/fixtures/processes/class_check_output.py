import subprocess


class Tool:
    library = subprocess

    def capture_stdout(self) -> bytes:
        return self.library.check_output(["/bin/sh", "-c", "printf ok"])


print(Tool().capture_stdout())
