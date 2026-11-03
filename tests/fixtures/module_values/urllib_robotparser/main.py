from urllib.robotparser import RobotFileParser

parser = RobotFileParser()
parser.parse(["User-agent: *", "Disallow: /", "User-agent: Indexer",
              "Disallow: /draft", "Allow: /draft/public", "Disallow: /query?$"])
assert not parser.can_fetch("Other", "/draft/public")
assert parser.can_fetch("Indexer/2", "/draft/public/page")
assert not parser.can_fetch("Indexer", "/draft/page")
assert parser.can_fetch("Indexer", "/query")
assert not parser.can_fetch("Indexer", "/query?")
