import { createReadStream } from "node:fs";
import { mkdir, writeFile } from "node:fs/promises";
import { dirname } from "node:path";
import { createInterface } from "node:readline";
import { createGunzip } from "node:zlib";

// Recognized release spellings and the data types of the pinned uv index.
const flavors = new Map([
  ["install_only_stripped", 0],
  ["install_only", 1],
  ["shared-pgo", 2],
  ["shared-noopt", 3],
  ["static-noopt", 4],
]);
const variants = [null, "freethreaded", "freethreaded+debug", "debug"];

function compare(a, b) {
  return a < b ? -1 : a > b ? 1 : 0;
}

function compareKeys(a, b) {
  for (let i = 0; i < a.length; i++) {
    const difference = compare(a[i], b[i]);
    if (difference) return difference;
  }
  return 0;
}

function parseVersion(text) {
  const match = /^(\d+)\.(\d+)\.(\d+)(?:(a|b|rc)(\d+))?$/.exec(text);
  if (!match) throw new Error("invalid version " + text);
  const [, major, minor, patch, kind = "", number = ""] = match;
  return {
    major: Number(major),
    minor: Number(minor),
    patch: Number(patch),
    prerelease: kind + number,
    sort: [
      -major,
      -minor,
      -patch,
      -["a", "b", "rc", ""].indexOf(kind),
      -Number(number),
    ],
  };
}

function parseTarget(platform, variant) {
  const options = new Set();
  for (const suffix of ["debug", "freethreaded"]) {
    if (platform.endsWith("-" + suffix)) {
      platform = platform.slice(0, -(suffix.length + 1));
      options.add(suffix);
    }
  }
  let flavor = variant;
  const parts = variant.split("+");
  if (
    parts.length > 1 &&
    ["full", "install_only", "install_only_stripped"].includes(parts.at(-1))
  ) {
    flavor = parts.pop();
    for (const part of parts) options.add(part);
  }
  if (options.has("static")) return null;
  const [machine, , os, env] = platform.split("-");
  if (os === undefined || (os === "linux" && env === undefined)) return null;
  const archName =
    machine === "ppc64"
      ? "powerpc64"
      : machine === "ppc64le"
        ? "powerpc64le"
        : machine;
  const archParts = archName.split("_");
  const family = archParts.slice(0, 2).join("_");
  const archVariant = archParts[2] ?? null;
  if (archVariant === "")
    throw new Error("architecture variants must not be empty");
  const archKey = archVariant === null ? family : family + "_" + archVariant;
  const name =
    ["freethreaded", "debug"]
      .filter((option) => options.has(option))
      .join("+") || null;
  const libc = os === "linux" ? env : "none";
  const priority = [
    flavors.get(flavor) ?? 6,
    -(1 + Number(options.has("lto")) + Number(options.has("pgo"))),
  ];
  return {
    os,
    arch: { family, variant: archVariant },
    archKey,
    libc,
    variant: name,
    priority,
    key: JSON.stringify([os, archKey, libc, name]),
    sort: [variants.indexOf(name), os, family, archVariant ?? "", libc],
  };
}

function selectRelease(groups, cache, record) {
  const [versionText, date] = record.version.split("+");
  const version = parseVersion(versionText);
  const release = Number(date);
  let group = groups.get(versionText);
  for (const artifact of record.artifacts.sort((a, b) =>
    compare(a.url, b.url),
  )) {
    const filename = decodeURIComponent(
      artifact.url.slice(artifact.url.lastIndexOf("/") + 1),
    );
    const labels = JSON.stringify([artifact.platform, artifact.variant]);
    let target = cache.get(labels);
    if (target === undefined) {
      target = parseTarget(artifact.platform, artifact.variant);
      if (target === null) continue;
      if (cache.size < 512) cache.set(labels, target);
    }
    if (release < 20250311 && target.libc === "musl") continue;
    if (!group) {
      group = new Map();
      groups.set(versionText, group);
    }
    const previous = group.get(target.key);
    if (previous && compareKeys(target.priority, previous.target.priority) >= 0)
      continue;
    group.set(target.key, {
      version,
      target,
      filename,
      url: artifact.url,
      sha256: artifact.sha256 ?? null,
      build: String(release),
      sort: [...version.sort, ...target.sort],
    });
  }
}

function finish(groups) {
  const selected = Array.from(groups.values()).flatMap((group) =>
    Array.from(group.values()),
  );
  const results = {};
  for (const download of selected.sort((a, b) => compareKeys(a.sort, b.sort))) {
    const { version, target, url, sha256, build } = download;
    const { major, minor, patch, prerelease } = version;
    if (major < 3 || (major === 3 && minor < 8)) continue;
    const { arch, os, libc, variant } = target;
    const name = "cpython";
    const prefix = name + "-" + major + "." + minor + "." + patch + prerelease;
    const key =
      prefix +
      (variant ? "+" + variant : "") +
      "-" +
      os +
      "-" +
      target.archKey +
      "-" +
      libc;
    results[key] = {
      name,
      arch,
      os,
      libc,
      major,
      minor,
      patch,
      prerelease,
      url,
      sha256,
      variant,
      build,
    };
  }
  return results;
}

if (process.argv.length !== 4)
  throw new Error("expected INPUT.ndjson[.gz] OUTPUT.json");
const [source, destination] = process.argv.slice(2);
const stream = createReadStream(source);
const input = source.endsWith(".gz") ? stream.pipe(createGunzip()) : stream;
const groups = new Map();
const cache = new Map();
for await (const line of createInterface({ input, crlfDelay: Infinity })) {
  if (line.trim()) selectRelease(groups, cache, JSON.parse(line));
}
const rendered = JSON.stringify(finish(groups), null, 2) + "\n";
await mkdir(dirname(destination), { recursive: true });
await writeFile(destination, rendered);
