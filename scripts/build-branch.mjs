#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { resolve } from "node:path";

const root = fileURLToPath(new URL("../", import.meta.url));
const packagePath = resolve(root, "package.json");
const packageJson = JSON.parse(readFileSync(packagePath, "utf8"));
const baseVersion = packageJson.version.split("-")[0];
const parts = new Intl.DateTimeFormat("en-GB", {
  timeZone: "Asia/Shanghai",
  year: "numeric",
  month: "2-digit",
  day: "2-digit",
  hour: "2-digit",
  minute: "2-digit",
  second: "2-digit",
  hourCycle: "h23",
}).formatToParts(new Date());
const timestamp = ["year", "month", "day", "hour", "minute", "second"]
  .map((type) => parts.find((part) => part.type === type).value)
  .join("");
const version = `${baseVersion}-kimicode.${timestamp}`;
packageJson.version = version;
writeFileSync(packagePath, `${JSON.stringify(packageJson, null, 2)}\n`);
const configPath = resolve(root, "src-tauri/tauri.conf.json");
const config = JSON.parse(readFileSync(configPath, "utf8"));
config.version = version;
writeFileSync(configPath, `${JSON.stringify(config, null, 2)}\n`);
const cargoPath = resolve(root, "src-tauri/Cargo.toml");
writeFileSync(
  cargoPath,
  readFileSync(cargoPath, "utf8").replace(
    /^version = "[^"]+"/m,
    `version = "${version}"`,
  ),
);
console.log(`Building ${version} (Asia/Shanghai)`);
const result = spawnSync(
  "pnpm",
  [
    "tauri",
    "build",
    "-c",
    JSON.stringify({ bundle: { createUpdaterArtifacts: false } }),
  ],
  {
    cwd: root,
    stdio: "inherit",
    env: { ...process.env, VITE_CC_SWITCH_DISABLE_AUTO_UPDATE: "1" },
  },
);
if (result.error) console.error(result.error.message);
process.exit(result.status ?? 1);
