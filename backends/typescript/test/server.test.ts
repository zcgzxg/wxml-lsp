import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdirSync, mkdtempSync, rmSync, symlinkSync } from "node:fs";
import { tmpdir } from "node:os";
import path from "node:path";
import { test } from "node:test";
import { BACKEND_DIR, SAMPLES_DIR } from "../src/env.ts";
import { inspectRequest, type RunConfig } from "../src/run.ts";
import { MAX_LINE_BYTES, resolveBackendDir } from "../src/server.ts";
import { BACKEND_VERSION } from "../src/version.ts";
import { scenarioById } from "../src/scenarios.ts";

const config: RunConfig = { backendDir: BACKEND_DIR, samplesDir: SAMPLES_DIR };
const SERVER = "src/server.ts";

interface Client {
  next(timeoutMs?: number): Promise<Record<string, unknown>>;
  send(value: unknown): void;
  sendRaw(line: string): void;
  stderr(): string;
  waitExit(timeoutMs?: number): Promise<number | null>;
  kill(): void;
}

function startServer(entry = SERVER): Client {
  const child = spawn(process.execPath, [entry], {
    cwd: BACKEND_DIR,
    stdio: ["pipe", "pipe", "pipe"],
  });
  let pending = "";
  let stderr = "";
  const messages: Array<Record<string, unknown>> = [];
  const waiters: Array<(message: Record<string, unknown>) => void> = [];

  child.stdout.setEncoding("utf8");
  child.stdout.on("data", (chunk: string) => {
    pending += chunk;
    let index = pending.indexOf("\n");
    while (index >= 0) {
      const line = pending.slice(0, index);
      pending = pending.slice(index + 1);
      index = pending.indexOf("\n");
      if (line.trim().length === 0) continue;
      const message = JSON.parse(line) as Record<string, unknown>;
      const waiter = waiters.shift();
      if (waiter) waiter(message);
      else messages.push(message);
    }
  });
  child.stderr.setEncoding("utf8");
  child.stderr.on("data", (chunk: string) => {
    stderr += chunk;
  });
  const exited = new Promise<number | null>((resolve) => child.on("exit", (code) => resolve(code)));

  return {
    next(timeoutMs = 15000) {
      if (messages.length > 0) return Promise.resolve(messages.shift()!);
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => reject(new Error("timeout waiting for server response")), timeoutMs);
        waiters.push((message) => {
          clearTimeout(timer);
          resolve(message);
        });
      });
    },
    send(value) {
      child.stdin.write(`${JSON.stringify(value)}\n`);
    },
    sendRaw(line) {
      child.stdin.write(line);
    },
    stderr: () => stderr,
    waitExit: (timeoutMs = 15000) =>
      Promise.race([
        exited,
        new Promise<never>((_, reject) => setTimeout(() => reject(new Error("server did not exit")), timeoutMs)),
      ]),
    kill() {
      child.kill("SIGKILL");
    },
  };
}

async function withServer(run: (client: Client) => Promise<void>): Promise<void> {
  const client = startServer();
  try {
    await run(client);
  } finally {
    client.kill();
  }
}

test("ping and shutdown follow JSON-RPC 2.0", async () => {
  await withServer(async (client) => {
    client.send({ jsonrpc: "2.0", id: 1, method: "ping" });
    const ping = await client.next();
    assert.equal(ping.jsonrpc, "2.0");
    assert.equal(ping.id, 1);
    assert.deepEqual(ping.result, { ok: true, protocolVersion: 2, backendVersion: BACKEND_VERSION });

    client.send({ jsonrpc: "2.0", id: 2, method: "shutdown" });
    const shutdown = await client.next();
    assert.deepEqual(shutdown.result, { ok: true });
    assert.equal(await client.waitExit(), 0);
  });
});

test("analyze returns the C1 response for a page", async () => {
  await withServer(async (client) => {
    const request = inspectRequest(scenarioById("page-basic"), "in-place-shadow", config);
    client.send({ jsonrpc: "2.0", id: "a1", method: "analyze", params: request });
    const response = await client.next();
    assert.equal(response.id, "a1");
    const result = response.result as {
      protocolVersion: number;
      requestId: string;
      fragments: Array<{ fragmentId: string; hover?: { typeOnly?: string }; diagnostics?: Array<{ range?: unknown }> }>;
    };
    assert.equal(result.protocolVersion, 2);
    assert.equal(result.requestId, request.requestId);
    const title = result.fragments.find((fragment) => fragment.fragmentId === "title");
    assert.equal(title?.hover?.typeOnly, "string");
    const typo = result.fragments.find((fragment) => fragment.fragmentId === "user.nmae");
    assert.deepEqual(typo?.diagnostics?.[0]?.range, { start: 134, end: 138 });
  });
});

test("unknown methods, bad JSON and bad params do not kill the server", async () => {
  await withServer(async (client) => {
    client.send({ jsonrpc: "2.0", id: 3, method: "nope" });
    const unknown = await client.next();
    assert.equal((unknown.error as { code?: number }).code, -32601);

    client.sendRaw("{not json}\n");
    const parse = await client.next();
    assert.equal((parse.error as { code?: number }).code, -32700);
    assert.equal(parse.id, null);

    client.send({ jsonrpc: "2.0", id: 4, method: "analyze", params: { protocolVersion: 2 } });
    const invalid = await client.next();
    assert.equal((invalid.error as { code?: number }).code, -32602);

    client.send({ jsonrpc: "2.0", id: 5, method: "ping" });
    const ping = await client.next();
    assert.equal((ping.result as { ok?: boolean }).ok, true);
  });
});

test("JSON-RPC notifications get no response", async () => {
  await withServer(async (client) => {
    client.send({ jsonrpc: "2.0", method: "nope" });
    client.send({ jsonrpc: "2.0", method: "analyze", params: { protocolVersion: 2 } });
    const request = inspectRequest(scenarioById("page-basic"), "in-place-shadow", config);
    request.fragments = [
      {
        fragmentId: "bad",
        exprText: "1 + 2",
        wxmlRange: { start: 0, end: 5 },
        fragmentRevision: 1,
        root: "page",
      },
    ];
    client.send({ jsonrpc: "2.0", method: "analyze", params: request });
    client.send({ jsonrpc: "2.0", id: 7, method: "ping" });
    const message = await client.next();
    assert.equal(message.id, 7, "the first reply must belong to the ping");
  });
});

test("malformed requests are rejected as invalid params, not internal errors", async () => {
  await withServer(async (client) => {
    const request = inspectRequest(scenarioById("page-basic"), "in-place-shadow", config);
    request.files[0]!.path = "../evil.ts";
    request.files[0]!.fileId = "../evil.ts";
    client.send({ jsonrpc: "2.0", id: 8, method: "analyze", params: request });
    const response = await client.next();
    assert.equal((response.error as { code?: number }).code, -32602);
    assert.equal((response.error as { data?: { code?: string } }).data?.code, "bad-params");

    client.send({ jsonrpc: "2.0", id: 9, method: "analyze", params: null });
    const nullParams = await client.next();
    assert.equal((nullParams.error as { code?: number }).code, -32602);

    const deep = `${"(".repeat(300)}title${")".repeat(300)}`;
    const deepRequest = inspectRequest(scenarioById("page-basic"), "in-place-shadow", config);
    deepRequest.fragments = [
      {
        fragmentId: "deep",
        exprText: deep,
        wxmlRange: { start: 0, end: Buffer.byteLength(deep, "utf8") },
        fragmentRevision: 1,
        root: "page",
      },
    ];
    client.send({ jsonrpc: "2.0", id: 10, method: "analyze", params: deepRequest });
    const deepResponse = await client.next();
    assert.equal((deepResponse.error as { code?: number }).code, -32602);
    assert.equal((deepResponse.error as { data?: { code?: string } }).data?.code, "bad-fragment");
  });
});

test("oversized lines are rejected and dropped without killing the server", async () => {
  await withServer(async (client) => {
    client.sendRaw(`${"x".repeat(MAX_LINE_BYTES + 16)}\n`);
    const rejected = await client.next(30000);
    assert.equal((rejected.error as { code?: number }).code, -32600);
    assert.equal((rejected.error as { data?: { code?: string } }).data?.code, "bad-limit");

    client.send({ jsonrpc: "2.0", id: 6, method: "ping" });
    const ping = await client.next();
    assert.equal((ping.result as { ok?: boolean }).ok, true);
  });
});

test("ten concurrent requests are all answered on the same stream", async () => {
  await withServer(async (client) => {
    const request = inspectRequest(scenarioById("page-basic"), "in-place-shadow", config);
    const ids: number[] = [];
    for (let index = 0; index < 10; index += 1) {
      ids.push(index);
      client.send(
        index % 2 === 0
          ? { jsonrpc: "2.0", id: index, method: "ping" }
          : { jsonrpc: "2.0", id: index, method: "analyze", params: request },
      );
    }
    const seen = new Set<number>();
    for (let index = 0; index < ids.length; index += 1) {
      const message = await client.next(30000);
      seen.add(message.id as number);
    }
    assert.deepEqual([...seen].sort((left, right) => left - right), ids);
  });
});

test("backend directory resolution supports source, dist and packaged layouts", () => {
  const root = mkdtempSync(path.join(tmpdir(), "wxml-lsp-backend-dir-"));
  const marker = (directory: string, create: boolean): void => {
    const modules = path.join(directory, "node_modules", "typescript");
    if (create) mkdirSync(modules, { recursive: true });
    else mkdirSync(directory, { recursive: true });
  };
  try {
    const source = path.join(root, "src");
    marker(root, true);
    mkdirSync(source, { recursive: true });
    assert.equal(resolveBackendDir(source), root);

    const dist = path.join(root, "dist");
    mkdirSync(dist, { recursive: true });
    assert.equal(resolveBackendDir(dist), root);

    const packaged = path.join(root, "package", "bin", "wxml-type-backend");
    marker(packaged, true);
    mkdirSync(path.join(root, "package", "bin"), { recursive: true });
    assert.equal(resolveBackendDir(packaged), packaged);

    const missing = path.join(root, "missing", "src");
    mkdirSync(missing, { recursive: true });
    assert.equal(resolveBackendDir(missing), path.dirname(missing));
  } finally {
    rmSync(root, { recursive: true, force: true });
  }
});

test("entry detection follows a symlinked install path", async () => {
  const directory = mkdtempSync(path.join(tmpdir(), "wxml-lsp-server-entry-"));
  const linked = path.join(directory, "server.ts");
  symlinkSync(path.resolve(BACKEND_DIR, SERVER), linked);
  const client = startServer(linked);
  try {
    client.send({ jsonrpc: "2.0", id: "symlink", method: "ping" });
    const response = await client.next();
    assert.equal(response.id, "symlink");
    assert.equal((response.result as { ok?: boolean }).ok, true);
  } finally {
    client.kill();
    rmSync(directory, { recursive: true, force: true });
  }
});
