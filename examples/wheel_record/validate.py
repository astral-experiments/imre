import sys
from zipfile import ZipFile

from installer.sources import WheelFile


with ZipFile(sys.argv[1]) as archive:
    WheelFile(archive).validate_record()
