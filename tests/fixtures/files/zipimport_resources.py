import zipimport
from pathlib import Path


for archive in ['stored.zip', 'deflated.zip']:
    loader = zipimport.zipimporter(archive + '/unused')
    assert isinstance(loader, zipimport.zipimporter)
    assert loader.archive == archive
    assert loader.prefix == 'unused/'
    name = 'pkg/templates/page.html'
    print(loader.get_data(name))
    print(loader.get_data(archive + '/' + name))
    print(loader.get_data('pkg/templates/'))
    try:
        loader.get_data('missing')
    except OSError as error:
        print((error.errno, error.strerror, error.filename))

    print(zipimport.zipimporter(archive + ".short-size").get_data(name))
    Path(archive).write_bytes(Path(archive + ".replacement").read_bytes())
    print(loader.get_data(name))
    Path(archive).write_bytes(b"short")
    try:
        loader.get_data(name)
    except EOFError as error:
        print(str(error))

try:
    zipimport.zipimporter('input.txt')
except zipimport.ZipImportError as error:
    assert isinstance(error, ImportError)
    print(str(error))
    print(error.path)
