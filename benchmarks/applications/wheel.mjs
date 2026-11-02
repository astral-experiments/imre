import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import { parse } from "csv-parse/sync";
import { unzipSync } from "fflate";

const members = unzipSync(readFileSync(process.argv[2]));
const records = Object.keys(members).filter((name) =>
  /^[^/]+\.dist-info\/RECORD$/.test(name),
);
if (records.length !== 1) throw new Error("expected one RECORD");
const [record] = records;
const rows = new Map(
  parse(new TextDecoder().decode(members[record])).map(([name, hash, size]) => [
    name,
    [hash, size],
  ]),
);
for (const [name, data] of Object.entries(members)) {
  if (name.endsWith("/")) continue;
  if (name === record + ".jws" || name === record + ".p7s") {
    if (rows.has(name)) throw new Error("signature in RECORD");
    continue;
  }
  if (!rows.has(name)) throw new Error("missing RECORD entry: " + name);
  const [hash, size] = rows.get(name);
  rows.delete(name);
  if (name === record) {
    if (hash || size) throw new Error("RECORD must have empty hash and size");
  } else if (
    !size ||
    Number(size) !== data.length ||
    hash !== "sha256=" + createHash("sha256").update(data).digest("base64url")
  ) {
    throw new Error("hash or size mismatch: " + name);
  }
}
