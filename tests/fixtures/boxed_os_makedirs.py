def current():
    import os
    import shutil
    import tempfile

    root = tempfile.mkdtemp()
    try:
        os.mkdir(root + '/default')
        os.mkdir(root + '/leaf-mode', 0o700)
        name = os.fsencode(root) + b'/parent/leaf'
        assert os.path.split(name) == (os.fsencode(root) + b'/parent', b'leaf')
        assert os.makedirs(name, mode=0o700) is None
        assert os.stat(name).st_mode == os.stat(root + '/leaf-mode').st_mode
        assert os.stat(os.fsencode(root) + b'/parent').st_mode == os.stat(root + '/default').st_mode
        assert os.makedirs(name, exist_ok=True) is None
        try:
            os.makedirs(name)
        except FileExistsError as error:
            assert error.filename == name
        else:
            raise AssertionError('existing directory accepted')
    finally:
        shutil.rmtree(root)
    print('makedirs')
