/** Paths and environment facts that the evidence record has to state. */

import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const BACKEND_DIR = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
export const SAMPLES_DIR = path.join(BACKEND_DIR, "samples");

export interface EnvironmentFacts {
  node: string;
  typescript: string;
  typings: string;
  platform: string;
  cpu: string;
  cpuCount: number;
  memoryGiB: number;
}

export function environmentFacts(): EnvironmentFacts {
  const versions = installedVersions();
  const cpus = os.cpus();
  return {
    node: process.version,
    typescript: versions.typescript,
    typings: versions.typings,
    platform: `${os.platform()} ${os.release()} (${os.arch()})`,
    cpu: cpus[0]?.model.trim() ?? "unknown",
    cpuCount: cpus.length,
    memoryGiB: Math.round((os.totalmem() / 1024 ** 3) * 10) / 10,
  };
}

/** Read the installed versions so the record never drifts from package.json. */
export function installedVersions(): { typescript: string; typings: string } {
  const read = (name: string): string => {
    const manifest = path.join(BACKEND_DIR, "node_modules", name, "package.json");
    if (!fs.existsSync(manifest)) return "not-installed";
    const parsed = JSON.parse(fs.readFileSync(manifest, "utf8")) as { version?: string };
    return parsed.version ?? "unknown";
  };
  return { typescript: read("typescript"), typings: read("miniprogram-api-typings") };
}
