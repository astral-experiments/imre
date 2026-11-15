rawdata = "<!--comment-->"
startswith = rawdata.startswith
print(startswith.__self__ is rawdata)
del rawdata
print(startswith("<", 0), startswith("</", 0), startswith("<!--", 0))
