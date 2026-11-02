def current():
    import os
    import posix
    import shutil
    import tempfile

    root = tempfile.mkdtemp()
    previous = os.umask(0o027)
    try:
        assert os.umask is posix.umask
        assert posix.umask(0o027) == 0o027
        os.mkdir(root + '/default')
        assert os.stat(root + '/default').st_mode & 0o777 == 0o750
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
        os.removedirs(name)
        assert not os.path.exists(os.path.join(root, 'parent'))
        assert os.path.isdir(root)
    finally:
        os.umask(previous)
        shutil.rmtree(root)
    print('makedirs')
