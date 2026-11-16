#!/usr/bin/env node
/**
 * P5 Node type backend: NDJSON JSON-RPC 2.0 over stdio.
 *
 * Methods: `ping`, `analyze` (C1 params -> C1 response), `shutdown`.
 * stdout carries protocol lines only; logs go to stderr. A single line is
 * capped at 20 MiB and an oversized line is rejected without buffering it.
 */

import path from "node:path";
import { fileURLToPath } from "node:url";
import { analyze } from "./analysis.ts";
import { complete } from "./completion.ts";
import {
  PROTOCOL_VERSION,
  RequestError,
  validateCompleteRequest,
  validateRequest,
  type AnalyzeRequest,
  type CompleteRequest,
} from "./protocol.ts";
import { BACKEND_VERSION } from "./version.ts";

/**
 * Line cap. Kept above the request `maxTotalBytes` (16 MiB) so a legal snapshot
 * is never rejected by the transport framing.
 */
export const MAX_LINE_BYTES = 20 * 1024 * 1024;

export interface JsonRpcError {
  code: number;
  message: string;
  data?: unknown;
}

export interface ServerOptions {
  input: NodeJS.ReadableStream;
  output: NodeJS.WritableStream;
  /** Compiler project root (holds node_modules with TypeScript and typings). */
  backendDir: string;
  maxLineBytes?: number;
  /** Log sink; defaults to stderr. */
  log?: (line: string) => void;
}

/**
 * Serve NDJSON JSON-RPC until `shutdown` or EOF.
 *
 * Resolves after the shutdown response has been flushed.
 */
export function serve(options: ServerOptions): Promise<void> {
  const maxLineBytes = options.maxLineBytes ?? MAX_LINE_BYTES;
  const log = options.log ?? ((line: string) => process.stderr.write(`${line}\n`));
  const write = (message: unknown): void => {
    options.output.write(`${JSON.stringify(message)}\n`);
  };
  const writeError = (
    id: string | number | null,
    code: number,
    message: string,
    data?: unknown,
  ): void => {
    const error: JsonRpcError = { code, message, ...(data === undefined ? {} : { data }) };
    write({ jsonrpc: "2.0", id, error });
  };

  return new Promise((resolve) => {
    let buffer: Buffer[] = [];
    let bufferedBytes = 0;
    let discarding = false;
    let shuttingDown = false;
    let stopped = false;

    const finish = (): void => {
      if (stopped) return;
      stopped = true;
      options.input.removeListener("data", onData);
      options.input.removeListener("end", onEnd);
      options.input.removeListener("error", onEnd);
      resolve();
    };

    const onEnd = (): void => finish();

    const flushLine = (line: Buffer): void => {
      if (shuttingDown) return;
      const text = line.toString("utf8").replace(/\r$/, "");
      if (text.trim().length === 0) return;
      handleLine(text);
    };

    const handleLine = (text: string): void => {
      let message: unknown;
      try {
        message = JSON.parse(text);
      } catch {
        writeError(null, -32700, "Parse error");
        return;
      }
      if (typeof message !== "object" || message === null) {
        writeError(null, -32600, "Invalid Request");
        return;
      }
      const request = message as {
        jsonrpc?: unknown;
        id?: unknown;
        method?: unknown;
        params?: unknown;
      };
      const hasId = Object.prototype.hasOwnProperty.call(request, "id");
      const id = typeof request.id === "string" || typeof request.id === "number" ? request.id : null;
      if (request.jsonrpc !== "2.0" || typeof request.method !== "string") {
        writeError(id, -32600, "Invalid Request");
        return;
      }
      const respond = (result: unknown): void => {
        if (!hasId) return;
        write({ jsonrpc: "2.0", id, result });
      };

      switch (request.method) {
        case "ping": {
          respond({ ok: true, protocolVersion: PROTOCOL_VERSION, backendVersion: BACKEND_VERSION });
          return;
        }
        case "analyze": {
          const params = request.params as AnalyzeRequest | undefined;
          if (typeof params !== "object" || params === null) {
            if (hasId) writeError(id, -32602, "Invalid params", { code: "bad-params" });
            return;
          }
          try {
            // Validate first so a malformed request never crashes on a field read.
            validateRequest(params);
          } catch (error) {
            if (!hasId) return;
            if (error instanceof RequestError) {
              log(`analyze request rejected: ${error.code}: ${error.message}`);
              writeError(id, -32602, "Invalid params", { code: error.code, message: error.message });
            } else if (error instanceof TypeError) {
              writeError(id, -32602, "Invalid params", { code: "bad-params" });
            } else {
              writeError(id, -32603, "Internal error");
            }
            return;
          }
          try {
            const result = analyze(params, { backendDir: options.backendDir });
            respond(result.response);
          } catch (error) {
            if (!hasId) return;
            if (error instanceof RequestError) {
              log(`analyze failed with request error: ${error.code}: ${error.message}`);
              writeError(id, -32602, "Invalid params", { code: error.code, message: error.message });
            } else {
              log(`analyze failed: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}`);
              writeError(id, -32603, "Internal error");
            }
          }
          return;
        }
        case "complete": {
          const params = request.params as CompleteRequest | undefined;
          if (typeof params !== "object" || params === null) {
            if (hasId) writeError(id, -32602, "Invalid params", { code: "bad-params" });
            return;
          }
          try {
            validateCompleteRequest(params);
          } catch (error) {
            if (!hasId) return;
            if (error instanceof RequestError) {
              writeError(id, -32602, "Invalid params", { code: error.code, message: error.message });
            } else if (error instanceof TypeError) {
              writeError(id, -32602, "Invalid params", { code: "bad-params" });
            } else {
              writeError(id, -32603, "Internal error");
            }
            return;
          }
          try {
            respond(complete(params, { backendDir: options.backendDir }));
          } catch (error) {
            if (!hasId) return;
            if (error instanceof RequestError) {
              writeError(id, -32602, "Invalid params", { code: error.code, message: error.message });
            } else {
              log(`complete failed: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}`);
              writeError(id, -32603, "Internal error");
            }
          }
          return;
        }
        case "shutdown": {
          respond({ ok: true });
          shuttingDown = true;
          // Flush, then resolve. The CLI exits after `serve` resolves.
          const output = options.output as { write?: (chunk: string, cb?: () => void) => boolean };
          if (typeof output.write === "function") {
            output.write("", () => setImmediate(finish));
          } else {
            setImmediate(finish);
          }
          return;
        }
        default: {
          if (!hasId) return;
          writeError(id, -32601, `Method not found: ${request.method}`);
        }
      }
    };

    function onData(chunk: Buffer | string): void {
      const data = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk, "utf8");
      let start = 0;
      for (let index = 0; index < data.length; index += 1) {
        if (data[index] !== 0x0a) continue;
        const piece = data.subarray(start, index);
        start = index + 1;
        if (discarding) {
          discarding = false;
          buffer = [];
          bufferedBytes = 0;
          continue;
        }
        if (bufferedBytes + piece.length > maxLineBytes) {
          buffer = [];
          bufferedBytes = 0;
          writeError(null, -32600, "Invalid Request", { code: "bad-limit", limit: maxLineBytes });
          continue;
        }
        const line = Buffer.concat([...buffer, piece]);
        buffer = [];
        bufferedBytes = 0;
        flushLine(line);
      }
      if (start < data.length) {
        const rest = data.subarray(start);
        if (discarding) return;
        if (bufferedBytes + rest.length > maxLineBytes) {
          buffer = [];
          bufferedBytes = 0;
          discarding = true;
          writeError(null, -32600, "Invalid Request", { code: "bad-limit", limit: maxLineBytes });
          return;
        }
        buffer.push(rest);
        bufferedBytes += rest.length;
      }
    }

    options.input.on("data", onData);
    options.input.on("end", onEnd);
    options.input.on("error", onEnd);
    if (typeof (options.input as { resume?: () => void }).resume === "function") {
      (options.input as { resume: () => void }).resume();
    }
  });
}

/** CLI entry: `node dist/server.js` (stdio). */
export async function main(): Promise<void> {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const backendDir = path.resolve(here, "..");
  await serve({ input: process.stdin, output: process.stdout, backendDir });
  process.exit(0);
}

const entry = process.argv[1] ? path.resolve(process.argv[1]) : "";
if (entry && entry === fileURLToPath(import.meta.url)) {
  main().catch((error: unknown) => {
    process.stderr.write(`server failed: ${error instanceof Error ? (error.stack ?? error.message) : String(error)}\n`);
    process.exit(1);
  });
}

export { PROTOCOL_VERSION };
