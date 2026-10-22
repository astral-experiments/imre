from relar.toml import decode as decode_toml
from models import Group
import facade

groups = decode_toml('[first]\nname="one"\nhref="/one"', dict[str, Group])
print(groups["first"].name)
print(facade.parse('name="two"\nhref="/two"').href)
