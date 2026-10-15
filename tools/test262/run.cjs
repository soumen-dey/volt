#!/usr/bin/env node
"use strict";

const fs = require("node:fs");
const path = require("node:path");
const { spawnSync } = require("node:child_process");
const TestStream = require("test262-stream");

const root = path.resolve(__dirname, "..", "..");
const args = parseArgs(process.argv.slice(2));

async function main() {
  if (!args.test262) {
    throw new Error("Pass --test262 <path-to-Test262-checkout>.");
  }

  const test262Dir = path.resolve(args.test262);
  const enginePath = path.resolve(
    args.engine || path.join(root, "target", "debug", `js-engine${process.platform === "win32" ? ".exe" : ""}`),
  );
  if (!fs.existsSync(path.join(test262Dir, "test"))) {
    throw new Error(`Test262 checkout not found: ${test262Dir}`);
  }
  if (!fs.existsSync(enginePath)) {
    throw new Error(`Engine executable not found: ${enginePath}. Run cargo build first.`);
  }

  const paths = args.paths.length ? args.paths : ["test"];
  for (const testPath of paths) {
    const normalized = path.normalize(testPath);
    if (path.isAbsolute(normalized) || normalized === ".." || normalized.startsWith(`..${path.sep}`)) {
      throw new Error(`Test path must be relative to the Test262 checkout: ${testPath}`);
    }
  }

  const stream = new TestStream(test262Dir, { paths });
  const counts = { passed: 0, failed: 0, skipped: 0 };
  const failures = [];
  const skips = new Map();
  let cases = 0;

  for await (const test of stream) {
    cases += 1;
    const skippedFor = skipReason(test);
    if (skippedFor) {
      counts.skipped += 1;
      skips.set(skippedFor, (skips.get(skippedFor) || 0) + 1);
      continue;
    }

    const result = await runCase(enginePath, test);
    if (result.pass) {
      counts.passed += 1;
    } else {
      counts.failed += 1;
      failures.push({
        file: test.file,
        scenario: test.scenario,
        reason: result.reason,
        stderr: result.stderr.trim().slice(0, 2000),
      });
      if (failures.length <= 20) {
        process.stderr.write(`FAIL ${test.file} [${test.scenario}]: ${result.reason}\n`);
      }
    }
  }

  const report = {
    test262Revision: gitRevision(test262Dir),
    engine: "Boa 0.22.0 embedded by js-engine",
    selection: paths,
    scenarios: cases,
    ...counts,
    skipReasons: Object.fromEntries(skips),
    failures,
  };
  if (args.report) {
    const reportPath = path.resolve(args.report);
    fs.writeFileSync(reportPath, `${JSON.stringify(report, null, 2)}\n`);
    process.stdout.write(`Report: ${reportPath}\n`);
  }
  process.stdout.write(
    `Test262 scenarios: ${cases}; passed: ${counts.passed}; failed: ${counts.failed}; skipped: ${counts.skipped}\n`,
  );
  process.exitCode = counts.failed > 0 ? 1 : 0;
}

async function runCase(enginePath, test) {
  const negative = test.attrs.negative;
  const source = test.attrs.flags.raw ? test.contents : addHarnessBindings(test);
  const result = spawnSync(enginePath, [], {
    cwd: root,
    encoding: "utf8",
    input: source,
    timeout: 10_000,
    maxBuffer: 4 * 1024 * 1024,
    windowsHide: true,
  });
  const stderr = result.stderr || "";

  if (!negative) {
    return result.status === 0
      ? { pass: true }
      : {
          pass: false,
          reason: result.error ? result.error.message : `exit ${result.status}`,
          stderr,
        };
  }
  if (result.error) {
    return { pass: false, reason: result.error.message, stderr };
  }
  if (result.status === 0) {
    return { pass: false, reason: "expected an uncaught exception but execution succeeded", stderr };
  }
  if (negative.type && !stderr.includes(negative.type)) {
    return {
      pass: false,
      reason: `expected ${negative.type}, received a different failure`,
      stderr,
    };
  }
  return { pass: true };
}

function addHarnessBindings(test) {
  const prelude = `
void Object.defineProperty(globalThis, "$262", {
  configurable: true,
  writable: true,
  value: { global: globalThis }
});
`;
  const insertionIndex = Math.max(0, test.insertionIndex);
  return `${test.contents.slice(0, insertionIndex)}${prelude}${test.contents.slice(insertionIndex)}`;
}

function skipReason(test) {
  const flags = test.attrs.flags || {};
  if (flags.async) return "async Test262 host protocol is not implemented";
  if (flags.module) return "Test262 module-fixture execution is not implemented";
  if ((test.attrs.features || []).includes("IsHTMLDDA")) {
    return "IsHTMLDDA is an HTML host feature, outside this runtime's scope";
  }
  if (/\$262\s*\.\s*(createRealm|detachArrayBuffer|evalScript|gc|agent)\b/.test(test.contents)) {
    return "Test262 realm, evalScript, GC, detach, and agent host APIs are not implemented";
  }
  if (/\$DONE\s*\(/.test(test.contents)) {
    return "Test262 asynchronous completion protocol is not implemented";
  }
  return null;
}

function gitRevision(directory) {
  const result = spawnSync("git", ["-C", directory, "rev-parse", "HEAD"], {
    encoding: "utf8",
    windowsHide: true,
  });
  return result.status === 0 ? result.stdout.trim() : "unknown";
}

function parseArgs(argv) {
  const parsed = { paths: [], test262: null, engine: null, report: null };
  for (let index = 0; index < argv.length; index += 1) {
    const arg = argv[index];
    if (!["--test262", "--engine", "--report", "--path"].includes(arg)) {
      throw new Error(`Unknown argument: ${arg}`);
    }
    const value = argv[index + 1];
    if (!value || value.startsWith("--")) {
      throw new Error(`${arg} requires a value.`);
    }
    index += 1;
    if (arg === "--path") parsed.paths.push(value);
    else if (arg === "--test262") parsed.test262 = value;
    else if (arg === "--engine") parsed.engine = value;
    else parsed.report = value;
  }
  return parsed;
}

main().catch((error) => {
  process.stderr.write(`${error.stack || error}\n`);
  process.exitCode = 2;
});
