from sys import argv

from conformance_report.app import main
from conformance_report.live import live_main

live_mode = False
for argument in argv:
    if argument == "--suite" or argument == "--old-ty" or argument == "--new-ty":
        live_mode = True

if live_mode:
    live_main(argv)
else:
    main(argv)
