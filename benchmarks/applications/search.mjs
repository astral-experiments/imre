import { readFileSync } from "node:fs";

const paths = readFileSync(process.argv[2], "utf8").trimEnd().split("\n");
for (const path of paths) {
  const lines = readFileSync(path, "utf8").split("\n");
  for (let i = 0; i < lines.length; i++) {
    if (lines[i].includes("TODO")) {
      console.log(`${path}:${i + 1}`);
    }
  }
}
