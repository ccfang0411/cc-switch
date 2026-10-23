#!/usr/bin/env node
import { existsSync, readFileSync, writeFileSync } from "node:fs";
import { spawnSync } from "node:child_process";
import { homedir } from "node:os";
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
// 私有更新通道的签名私钥：优先用环境变量（可为路径或内容），否则用默认位置。
// 私钥绝不能提交到仓库；丢失后已安装的客户端将无法再收到更新。
const defaultSigningKey = resolve(homedir(), ".tauri/cc-switch-updater.key");
const configuredKey = process.env.TAURI_SIGNING_PRIVATE_KEY;
let signingKeyPath = "";
let signingKeyContent = "";
if (configuredKey) {
  if (existsSync(configuredKey)) signingKeyPath = configuredKey;
  else signingKeyContent = configuredKey;
} else if (existsSync(defaultSigningKey)) {
  signingKeyPath = defaultSigningKey;
}
if (!signingKeyPath && !signingKeyContent) {
  console.error(
    [
      "缺少更新签名私钥，无法生成可用的更新包。",
      "先生成一次：pnpm tauri signer generate -w ~/.tauri/cc-switch-updater.key -p \"\"",
      "或用 TAURI_SIGNING_PRIVATE_KEY 指定私钥路径/内容。",
    ].join("\n"),
  );
  process.exit(1);
}
// 公钥抄错一个字符就会让所有客户端验签失败，构建前必须逐字符比对。
if (signingKeyPath && existsSync(`${signingKeyPath}.pub`)) {
  const expectedPubkey = readFileSync(`${signingKeyPath}.pub`, "utf8").trim();
  const configuredPubkey = JSON.parse(
    readFileSync(resolve(root, "src-tauri/tauri.conf.json"), "utf8"),
  )?.plugins?.updater?.pubkey?.trim();
  if (configuredPubkey !== expectedPubkey) {
    console.error(
      "tauri.conf.json 的 plugins.updater.pubkey 与签名私钥不匹配，构建已中止。",
    );
    process.exit(1);
  }
}
const env = {
  ...process.env,
  TAURI_SIGNING_PRIVATE_KEY_PASSWORD:
    process.env.TAURI_SIGNING_PRIVATE_KEY_PASSWORD ?? "",
};
// Tauri 只认 TAURI_SIGNING_PRIVATE_KEY（内容），因此路径也先读成内容。
env.TAURI_SIGNING_PRIVATE_KEY = signingKeyPath
  ? readFileSync(signingKeyPath, "utf8")
  : signingKeyContent;
delete env.TAURI_SIGNING_PRIVATE_KEY_PATH;
const result = spawnSync(
  "pnpm",
  [
    "tauri",
    "build",
    "--bundles",
    "app",
    "-c",
    JSON.stringify({ bundle: { createUpdaterArtifacts: true } }),
  ],
  {
    cwd: root,
    stdio: "inherit",
    env,
  },
);
if (result.error) console.error(result.error.message);
if (result.status !== 0) process.exit(result.status ?? 1);

// Tauri 的 bundle_dmg.sh 在本机反复失败；改用 hdiutil 从已签名的 .app 直接出 DMG，
// 同时保留 --bundles app 生成的 .app.tar.gz 与 .sig 作为应用内更新包。
const arch = process.arch === "arm64" ? "aarch64" : "x64";
const appPath = resolve(
  root,
  "src-tauri/target/release/bundle/macos/CC Switch.app",
);
const dmgPath = resolve(
  root,
  `src-tauri/target/release/bundle/dmg/CC Switch_${version}_${arch}.dmg`,
);
const dmg = spawnSync(
  "hdiutil",
  [
    "create",
    "-volname",
    "CC Switch",
    "-srcfolder",
    appPath,
    "-ov",
    "-format",
    "UDZO",
    dmgPath,
  ],
  { stdio: "inherit" },
);
if (dmg.status !== 0) process.exit(dmg.status ?? 1);
const verify = spawnSync("hdiutil", ["verify", dmgPath], { stdio: "inherit" });
if (verify.status !== 0) process.exit(verify.status ?? 1);
console.log(`DMG: ${dmgPath}`);
process.exit(0);
