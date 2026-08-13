import { copyFileSync, mkdirSync, readdirSync, statSync, writeFileSync } from "node:fs";
import { basename, extname, join, relative } from "node:path";

const [source, output = "static/projectm/presets/en-d"] = process.argv.slice(2);
if (!source) throw new Error("usage: node prepare-presets.mjs <en-d-source> [output]");

function walk(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    return entry.isDirectory() ? walk(path) : [path];
  });
}

mkdirSync(output, { recursive: true });
const files = walk(source)
  .filter((path) => extname(path).toLowerCase() === ".milk")
  .sort((a, b) => relative(source, a).localeCompare(relative(source, b)));

const presets = files.map((path, index) => {
  const file = `${String(index + 1).padStart(3, "0")}.milk`;
  copyFileSync(path, join(output, file));
  return {
    id: file,
    file,
    name: basename(path, extname(path)),
    source: relative(source, path),
    bytes: statSync(path).size,
  };
});

writeFileSync(join(output, "index.json"), `${JSON.stringify({ pack: "En D", presets }, null, 2)}\n`);
console.log(`Prepared ${presets.length} En D presets in ${output}`);
