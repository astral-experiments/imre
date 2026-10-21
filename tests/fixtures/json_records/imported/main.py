import relar.json as wire
from relar import json
from models import Record as Entry
from decoder import decode as restored

first = wire.decode('{"name":"first"}', Entry)
second = json.decode('{"name":"second","count":11}', Entry)
third = restored('{"name":"third","count":-1}', Entry)
print(f'{first.name} {first.count} {second.name} {second.count} {third.name} {third.count}')
