#!/usr/bin/env node
// 发布私有更新：把 Tauri 生成的 .app.tar.gz + .sig、DMG 与 latest.json 上传到 OSS，
// 供应用内「检查更新」使用。所有更新对象都设为公共读（Tauri updater 匿名访问）。
//
// 用法：
//   pnpm publish:update [--notes "版本说明"] [--dmg 安装包路径] [--prefix cc-switch-updates]
// 凭证：OSS_ACCESS_KEY_ID / OSS_ACCESS_KEY_SECRET（可选 OSS_BUCKET / OSS_ENDPOINT）。
import { createHash } from "node:crypto";
import { copyFile, mkdtemp, readFile, readdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";
import { uploadArtifact } from "./upload-oss.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const bundleDir = resolve(root, "src-tauri/target/release/bundle");
const macosDir = resolve(bundleDir, "macos");
const dmgDir = resolve(bundleDir, "dmg");

function credentials() {
  return {
    bucket: process.env.OSS_BUCKET || "digital-attic",
    endpoint: process.env.OSS_ENDPOINT || "oss-cn-hangzhou.aliyuncs.com",
    accessKeyId:
      process.env.OSS_ACCESS_KEY_ID || process.env.ALIBABA_CLOUD_ACCESS_KEY_ID,
    accessKeySecret:
      process.env.OSS_ACCESS_KEY_SECRET ||
      process.env.ALIBABA_CLOUD_ACCESS_KEY_SECRET,
  };
}

async function sha256(file) {
  const hash = createHash("sha256");
  hash.update(await readFile(file));
  return hash.digest("hex");
}

async function currentDmg(version) {
  const matches = (await readdir(dmgDir)).filter(
    (name) => name.startsWith(`CC Switch_${version}_`) && name.endsWith(".dmg"),
  );
  if (matches.length !== 1) {
    throw new Error(
      `未找到唯一的当前版本 DMG（${matches.length} 个），请用 --dmg 指定，或先用 hdiutil 补打`,
    );
  }
  return resolve(dmgDir, matches[0]);
}

function platformKey(fileName, version) {
  const suffix = fileName
    .replace(`CC Switch_${version}_`, "")
    .replace(/\.dmg$/, "");
  if (suffix === "aarch64") return "darwin-aarch64";
  if (suffix === "x64" || suffix === "x86_64") return "darwin-x86_64";
  throw new Error(`无法从文件名判断平台：${fileName}`);
}

async function main() {
  const { values } = parseArgs({
    options: {
      notes: { type: "string" },
      dmg: { type: "string" },
      prefix: { type: "string" },
      help: { type: "boolean" },
    },
  });
  if (values.help) {
    console.log(
      "用法：pnpm publish:update [--notes 版本说明] [--dmg DMG路径] [--prefix OSS目录]\n" +
        "凭证：OSS_ACCESS_KEY_ID / OSS_ACCESS_KEY_SECRET",
    );
    return;
  }

  const conf = JSON.parse(
    await readFile(resolve(root, "src-tauri/tauri.conf.json"), "utf8"),
  );
  const version = conf.version;
  if (!version) throw new Error("tauri.conf.json 缺少版本号");

  const tar = resolve(macosDir, "CC Switch.app.tar.gz");
  const sig = `${tar}.sig`;
  const dmg = values.dmg ? resolve(values.dmg) : await currentDmg(version);
  const signature = (await readFile(sig, "utf8")).trim();
  if (!signature) throw new Error(`签名文件为空：${sig}`);

  const creds = credentials();
  const prefix = values.prefix || "cc-switch-updates";
  const publicRead = true;
  const upload = (file, options = {}) =>
    uploadArtifact({
      file,
      ...creds,
      prefix,
      publicRead,
      forbidOverwrite: true,
      ...options,
    });

  const tempDir = await mkdtemp(resolve(tmpdir(), "cc-switch-update-"));
  try {
    // OSS 上按版本号命名更新包，避免后续版本覆盖同名对象（forbidOverwrite）。
    const platform = platformKey(basename(dmg), version);
    const arch = platform.split("-")[1];
    const versionedTar = resolve(
      tempDir,
      `CC Switch_${version}_${arch}.app.tar.gz`,
    );
    const versionedSig = `${versionedTar}.sig`;
    await copyFile(tar, versionedTar);
    await copyFile(sig, versionedSig);

    console.log(`发布 ${version}`);
    const tarResult = await upload(versionedTar);
    const sigResult = await upload(versionedSig);
    const dmgResult = await upload(dmg);

    const manifest = {
      version,
      notes: values.notes || `CC Switch 私有版本 ${version}`,
      pub_date: new Date().toISOString(),
      platforms: {
        [platform]: {
          signature,
          url: tarResult.publicUrl,
        },
      },
    };

    const manifestPath = resolve(tempDir, "latest.json");
    await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    const manifestResult = await uploadArtifact({
      file: manifestPath,
      ...creds,
      prefix,
      publicRead,
      forbidOverwrite: false,
    });

    console.log(
      JSON.stringify(
        {
          version,
          manifest: manifestResult.publicUrl,
          updateBundle: tarResult.publicUrl,
          signature: sigResult.publicUrl,
          dmg: dmgResult.publicUrl,
          dmgSha256: await sha256(dmg),
          notes: manifest.notes,
        },
        null,
        2,
      ),
    );
  } finally {
    await rm(tempDir, { recursive: true, force: true });
  }
}

if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(resolve(process.argv[1])).href
) {
  main().catch((error) => {
    console.error(error.message);
    process.exitCode = 1;
  });
}