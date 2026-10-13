import { test } from "node:test";
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { createHash, createHmac } from "node:crypto";
import { mkdtemp, writeFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { uploadArtifact } from "./upload-oss.mjs";

async function fixture(t) {
  const directory = await mkdtemp(join(tmpdir(), "oss-upload-test-"));
  const file = join(directory, "CC Switch 测试.dmg");
  const content = Buffer.from("installer fixture");
  await writeFile(file, content);
  const digest = createHash("md5").update(content).digest();
  const state = { object: null, puts: 0, failPut: false, corrupt: false };
  const server = createServer(async (req, res) => {
    const url = new URL(req.url, "http://localhost");
    const resource = `/digital-attic${decodeURIComponent(url.pathname)}`;
    let value;
    if (url.searchParams.has("Signature")) {
      value = `GET\n\n\n${url.searchParams.get("Expires")}\n${resource}`;
      assert.equal(
        url.searchParams.get("Signature"),
        createHmac("sha1", "test-secret").update(value).digest("base64"),
      );
    } else {
      const oss = Object.keys(req.headers)
        .filter((key) => key.startsWith("x-oss-"))
        .sort()
        .map((key) => `${key}:${req.headers[key]}\n`)
        .join("");
      value = `${req.method}\n${req.headers["content-md5"] ?? ""}\n${req.headers["content-type"] ?? ""}\n${req.headers.date}\n${oss}${resource}`;
      assert.equal(
        req.headers.authorization,
        `OSS test-id:${createHmac("sha1", "test-secret").update(value).digest("base64")}`,
      );
    }
    if (req.method === "PUT") {
      state.puts++;
      assert.equal(req.headers["x-oss-forbid-overwrite"], "true");
      assert.equal(req.headers["content-md5"], digest.toString("base64"));
      if (state.failPut) {
        res.writeHead(403);
        res.end(
          "<Error><Code>AccessDenied</Code><Secret>not-for-logs</Secret></Error>",
        );
        return;
      }
      const chunks = [];
      for await (const chunk of req) chunks.push(chunk);
      state.object = Buffer.concat(chunks);
      assert.deepEqual(state.object, content);
      res.end();
    } else if (!state.object) {
      res.writeHead(404);
      res.end();
    } else if (req.method === "HEAD") {
      res.writeHead(200, {
        "Content-Length": state.object.length,
        ETag: `"${state.corrupt ? "bad-md5" : createHash("md5").update(state.object).digest("hex")}"`,
      });
      res.end();
    } else {
      assert.equal(req.headers.range, "bytes=0-0");
      res.writeHead(206);
      res.end(state.object.subarray(0, 1));
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  t.after(async () => {
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
    await rm(directory, { recursive: true, force: true });
  });
  const options = {
    file,
    bucket: "digital-attic",
    endpoint: "oss-cn-hangzhou.aliyuncs.com",
    accessKeyId: "test-id",
    accessKeySecret: "test-secret",
    prefix: "releases/测试",
    fetchImpl: (url, init) => {
      const target = new URL(url);
      return fetch(
        `http://127.0.0.1:${server.address().port}${target.pathname}${target.search}`,
        init,
      );
    },
  };
  return { options, state, content };
}

test("uploads binary data, verifies remote contents and checks a signed URL with spaces and Unicode", async (t) => {
  const { options, state } = await fixture(t);
  const result = await uploadArtifact(options);
  assert.equal(state.puts, 1);
  assert.equal(result.skipped, false);
  assert.ok(result.ossPath.includes("releases/测试/CC Switch 测试.dmg"));
  assert.ok(result.downloadUrl.includes("CC%20Switch%20"));
});

test("same content skips upload while conflicting content is never overwritten", async (t) => {
  const { options, state, content } = await fixture(t);
  state.object = content;
  assert.equal((await uploadArtifact(options)).skipped, true);
  assert.equal(state.puts, 0);
  state.object = Buffer.from("other artifact");
  await assert.rejects(uploadArtifact(options), /大小或 MD5 不一致/);
  assert.equal(state.puts, 0);
});

test("rejects denied uploads without dumping response secrets and detects verification failures", async (t) => {
  const { options, state } = await fixture(t);
  state.failPut = true;
  await assert.rejects(uploadArtifact(options), (error) => {
    assert.match(error.message, /403.*AccessDenied/);
    assert.ok(!error.message.includes("not-for-logs"));
    return true;
  });
  state.failPut = false;
  state.corrupt = true;
  await assert.rejects(uploadArtifact(options), /大小或 MD5 不一致/);
});

test("validates credentials, endpoint and expiry before contacting OSS", async () => {
  const options = {
    file: "/nonexistent",
    bucket: "digital-attic",
    endpoint: "oss-cn-hangzhou.aliyuncs.com",
    accessKeyId: "test-id",
    accessKeySecret: "test-secret",
  };
  await assert.rejects(
    uploadArtifact({ ...options, accessKeySecret: "" }),
    /环境变量/,
  );
  await assert.rejects(
    uploadArtifact({
      ...options,
      endpoint: "http://oss-cn-hangzhou.aliyuncs.com",
    }),
    /HTTPS/,
  );
  await assert.rejects(
    uploadArtifact({
      ...options,
      endpoint: "oss-cn-hangzhou.aliyuncs.com.evil.test",
    }),
    /HTTPS/,
  );
  await assert.rejects(
    uploadArtifact({ ...options, expiresSeconds: 0 }),
    /有效期/,
  );
});
