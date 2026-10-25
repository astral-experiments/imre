import logging

fmt = ""
for i in range(768):
    fmt += "%(asctime)s|"
logging.basicConfig(format=fmt)
for i in range(24):
    logging.getLogger().warning("duplicate clocks")
