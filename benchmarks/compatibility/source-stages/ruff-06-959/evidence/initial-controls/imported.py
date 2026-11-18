from ruff06_subject import main
from bad_status import BAD_STATUS

def launch() -> int:
    status: int = main()
    status = BAD_STATUS
    return status
