#!/usr/bin/env node
import { createHash, createHmac } from "node:crypto";
import { createReadStream } from "node:fs";
import { readFile, readdir, stat } from "node:fs/promises";
import { basename, resolve } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { parseArgs } from "node:util";

const root = fileURLToPath(new URL("../", import.meta.url));
const sign = (secret, value) =>
  createHmac("sha1", secret).update(value).digest("base64");

// OSS V1: https://help.aliyun.com/zh/oss/developer-reference/include-signatures-in-the-authorization-header
export function authorization(method, headers, resource, credentials) {
  const normalized = Object.fromEntries(
    Object.entries(headers).map(([name, value]) => [
      name.toLowerCase(),
      value.trim(),
    ]),
  );
  const ossHeaders = Object.keys(normalized)
    .filter((name) => name.startsWith("x-oss-"))
    .sort()
    .map((name) => `${name}:${normalized[name]}\n`)
    .join("");
  const value = [
    method,
    normalized["content-md5"] ?? "",
    normalized["content-type"] ?? "",
    normalized.date,
    ossHeaders + resource,
  ].join("\n");
  return `OSS ${credentials.accessKeyId}:${sign(credentials.accessKeySecret, value)}`;
}

export async function uploadArtifact({
  file,
  bucket,
  endpoint,
  accessKeyId,
  accessKeySecret,
  prefix = "",
  expiresSeconds = 86400,
  publicRead = false,
  forbidOverwrite = true,
  fetchImpl = fetch,
}) {
  if (!accessKeyId || !accessKeySecret)
    throw new Error(
      "请设置 OSS_ACCESS_KEY_ID 和 OSS_ACCESS_KEY_SECRET 环境变量",
    );
  if (!/^[a-z0-9][a-z0-9-]{1,61}[a-z0-9]$/.test(bucket ?? ""))
    throw new Error("OSS Bucket 名称无效");
  if (
    !Number.isInteger(expiresSeconds) ||
    expiresSeconds < 1 ||
    expiresSeconds > 604800
  ) {
    throw new Error("链接有效期须为 1 到 604800 秒");
  }
  const origin = new URL(
    endpoint.includes("://") ? endpoint : `https://${endpoint}`,
  );
  if (
    origin.protocol !== "https:" ||
    origin.username ||
    origin.password ||
    origin.port ||
    origin.pathname !== "/" ||
    origin.search ||
    origin.hash ||
    !/^(?:[a-z0-9-]+\.)?oss-[a-z0-9-]+\.aliyuncs\.com(?:\.cn)?$/.test(
      origin.hostname,
    )
  ) {
    throw new Error("Endpoint 须为阿里云 OSS HTTPS 地域地址");
  }
  if (!origin.hostname.startsWith(`${bucket}.`)) {
    if (!origin.hostname.startsWith("oss-"))
      throw new Error("Endpoint 中的 Bucket 与 OSS_BUCKET 不一致");
    origin.hostname = `${bucket}.${origin.hostname}`;
  }
  const key = [prefix.replace(/^\/+|\/+$/g, ""), basename(file)]
    .filter(Boolean)
    .join("/");
  if (key.split("/").some((part) => part === "." || part === ".."))
    throw new Error("OSS 目录不能包含 . 或 ..");
  const objectUrl = `${origin.origin}/${key.split("/").map(encodeURIComponent).join("/")}`;
  const resource = `/${bucket}/${key}`;
  const credentials = { accessKeyId, accessKeySecret };
  const info = await stat(file);
  if (!info.isFile() || info.size === 0 || info.size > 5 * 1024 ** 3)
    throw new Error("请选择非空且不超过 5 GiB 的安装包文件");
  const hash = createHash("md5");
  for await (const chunk of createReadStream(file)) hash.update(chunk);
  const digest = hash.digest();
  const md5 = digest.toString("hex");
  async function send(method, body, extra = {}, subresource = "") {
    const headers = { Date: new Date().toUTCString(), ...extra };
    headers.Authorization = authorization(
      method,
      headers,
      resource + subresource,
      credentials,
    );
    return fetchImpl(`${objectUrl}${subresource}`, {
      method,
      headers,
      body,
      ...(body ? { duplex: "half" } : {}),
      redirect: "error",
      signal: AbortSignal.timeout(600000),
    });
  }
  async function requireOk(response, action) {
    if (response.ok) return;
    const text = await response.text();
    const code = text.match(/<Code>\s*([A-Za-z0-9]+)\s*<\/Code>/)?.[1];
    throw new Error(
      `OSS ${action} 失败：HTTP ${response.status}${code ? ` (${code})` : ""}`,
    );
  }
  function verify(response) {
    const etag = response.headers
      .get("etag")
      ?.replaceAll('"', "")
      .toLowerCase();
    if (
      Number(response.headers.get("content-length")) !== info.size ||
      etag !== md5
    ) {
      throw new Error("OSS 同名文件的大小或 MD5 不一致，未覆盖文件");
    }
  }
  const existing = await send("HEAD");
  let skipped = false;
  if (existing.ok && forbidOverwrite) {
    verify(existing);
    skipped = true;
    await existing.body?.cancel();
  } else {
    if (!existing.ok && existing.status !== 404)
      await requireOk(existing, "查询文件");
    await existing.body?.cancel();
    const response = await send("PUT", createReadStream(file), {
      "Content-Type": file.endsWith(".dmg")
        ? "application/x-apple-diskimage"
        : "application/octet-stream",
      "Content-Length": String(info.size),
      "Content-MD5": digest.toString("base64"),
      "Content-Disposition": `attachment; filename*=UTF-8''${encodeURIComponent(basename(file))}`,
      ...(publicRead ? { "x-oss-object-acl": "public-read" } : {}),
      ...(forbidOverwrite ? { "x-oss-forbid-overwrite": "true" } : {}),
    });
    await requireOk(response, "上传");
    await response.body?.cancel();
    const remote = await send("HEAD");
    await requireOk(remote, "校验文件");
    verify(remote);
    await remote.body?.cancel();
  }
  if (publicRead && skipped) {
    // 同名文件已存在时内容不需要重传，但对象可能还是私有的，补一次 ACL。
    const acl = await send(
      "PUT",
      null,
      { "x-oss-object-acl": "public-read" },
      "?acl",
    );
    await requireOk(acl, "设置公共读");
    await acl.body?.cancel();
  }
  if (publicRead) {
    const publicReadCheck = await fetchImpl(objectUrl, {
      headers: { Range: "bytes=0-0" },
      redirect: "error",
      signal: AbortSignal.timeout(30000),
    });
    if (publicReadCheck.status !== 200 && publicReadCheck.status !== 206)
      throw new Error(
        `对象未生效为公共读（HTTP ${publicReadCheck.status}），请检查桶的公共访问限制或权限`,
      );
    await publicReadCheck.body?.cancel();
  }
  const expires = Math.floor(Date.now() / 1000) + expiresSeconds;
  const query = new URLSearchParams({
    OSSAccessKeyId: accessKeyId,
    Expires: String(expires),
    Signature: sign(accessKeySecret, `GET\n\n\n${expires}\n${resource}`),
  });
  const downloadUrl = `${objectUrl}?${query}`;
  const download = await fetchImpl(downloadUrl, {
    headers: { Range: "bytes=0-0" },
    redirect: "error",
    signal: AbortSignal.timeout(30000),
  });
  await requireOk(download, "检查下载链接");
  await download.body?.cancel();
  return {
    ossPath: `oss://${bucket}/${key}`,
    size: info.size,
    md5,
    skipped,
    publicRead,
    publicUrl: publicRead ? objectUrl : null,
    downloadUrl,
    expiresAt: new Date(expires * 1000).toISOString(),
  };
}

async function currentDmg() {
  const { version } = JSON.parse(
    await readFile(resolve(root, "package.json"), "utf8"),
  );
  const directory = resolve(root, "src-tauri/target/release/bundle/dmg");
  const matches = (await readdir(directory)).filter(
    (name) => name.startsWith(`CC Switch_${version}_`) && name.endsWith(".dmg"),
  );
  if (matches.length !== 1)
    throw new Error(
      "未找到唯一的当前版本 DMG，请先 pnpm build:branch 或使用 --file 指定文件",
    );
  return resolve(directory, matches[0]);
}

async function main() {
  const { values } = parseArgs({
    options: {
      file: { type: "string" },
      prefix: { type: "string" },
      expires: { type: "string" },
      help: { type: "boolean" },
    },
  });
  if (values.help) {
    console.log(
      "用法：pnpm upload:oss [--file 安装包路径] [--prefix OSS目录] [--expires 有效秒数]\n凭证：OSS_ACCESS_KEY_ID / OSS_ACCESS_KEY_SECRET\n可选：OSS_BUCKET / OSS_ENDPOINT（默认 digital-attic / oss-cn-hangzhou.aliyuncs.com）",
    );
    return;
  }
  const result = await uploadArtifact({
    file: values.file ? resolve(values.file) : await currentDmg(),
    bucket: process.env.OSS_BUCKET || "digital-attic",
    endpoint: process.env.OSS_ENDPOINT || "oss-cn-hangzhou.aliyuncs.com",
    accessKeyId:
      process.env.OSS_ACCESS_KEY_ID || process.env.ALIBABA_CLOUD_ACCESS_KEY_ID,
    accessKeySecret:
      process.env.OSS_ACCESS_KEY_SECRET ||
      process.env.ALIBABA_CLOUD_ACCESS_KEY_SECRET,
    prefix: values.prefix || "",
    expiresSeconds: Number(values.expires ?? 86400),
  });
  console.log(
    result.skipped
      ? "同名文件内容一致，已跳过上传并完成校验。"
      : "上传成功，文件大小和 MD5 校验通过。",
  );
  console.log(JSON.stringify(result, null, 2));
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
