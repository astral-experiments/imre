"""Consumer of shutil's boolean descriptor conversion and shared warning filters."""
import csv
import shutil
import sys
import warnings


def operation(name: str, path: str) -> None:
    try:
        if name == 'disk':
            usage = shutil.disk_usage(False)
            print(usage.total > 0)
        elif name == 'own':
            shutil.chown(False, -1, -1)
        elif name == 'directory':
            shutil.chown(path, -1, -1, dir_fd=False)
        elif name == 'both':
            shutil.chown(False, -1, -1, dir_fd=True)
        elif name == 'bad_uid':
            shutil.chown(False, 3.5, -1)
        elif name == 'no_ids':
            shutil.chown(False)
        elif name == 'bad_group':
            shutil.chown(False, -1, b'group')
        else:
            raise AssertionError(name)
        print('return')
    except csv.Error:
        print('csv handler')
    except shutil.Error:
        print('shutil handler')
    except OSError as error:
        print(repr(error))
        print(repr(error.filename))
    except RuntimeWarning as error:
        print(repr(error))
    except Warning:
        print('parent warning handler')
    except (TypeError, ValueError) as error:
        print(repr(error))


warnings.resetwarnings()
action = sys.argv[1]
if action == 'line':
    warnings.filterwarnings('ignore', category=RuntimeWarning, module='^shutil$', lineno=1490)
elif action == 'parent':
    warnings.simplefilter('error', Warning)
else:
    warnings.simplefilter(action, RuntimeWarning)
operation(sys.argv[2], sys.argv[3])
operation(sys.argv[2], sys.argv[3])
