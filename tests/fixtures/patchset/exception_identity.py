from unidiff import PatchSet, UnidiffParseError


try:
    raise StopIteration("ordinary exhaustion")
except UnidiffParseError:
    print("incorrectly caught by patch parsing")
except StopIteration as error:
    print(str(error))

try:
    PatchSet("@@ -1,2 +1,2 @@\n a\n b\n")
except StopIteration:
    print("incorrectly caught by iteration")
except UnidiffParseError as error:
    print(str(error))
