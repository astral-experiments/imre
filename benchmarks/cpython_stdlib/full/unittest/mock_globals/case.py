import observer
from observer import recorder as record

record.assert_called_once_with("during import")
record("through alias")
observer.recorder("through module", check=True)
record.assert_any_call("through alias")
record.assert_called_with("through module", check=True)
print(observer.recorder.call_count)
try:
    record.assert_called_once()
except AssertionError as exc:
    print(str(exc))
