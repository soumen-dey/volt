"use strict";

const test = require("node:test");
const assert = require("node:assert/strict");
const { parseArgs, skipReason } = require("../tools/test262/run.cjs");

test("runner requires values for options and accepts repeated test paths", () => {
  assert.deepEqual(
    parseArgs(["--test262", "C:\\test262", "--path", "test/language", "--path", "test/built-ins"]),
    {
      paths: ["test/language", "test/built-ins"],
      test262: "C:\\test262",
      engine: null,
      report: null,
      allowUnpinned: false,
    },
  );
  assert.throws(() => parseArgs(["--report"]), /requires a value/);
  assert.throws(() => parseArgs(["--not-an-option"]), /Unknown argument/);
});

test("runner requires explicit opt-in for unpinned exploratory runs", () => {
  assert.equal(parseArgs(["--allow-unpinned"]).allowUnpinned, true);
});

test("runner skips only tests requiring unsupported host facilities", () => {
  assert.equal(
    skipReason({ attrs: { flags: { async: true }, features: [] }, contents: "" }),
    "async Test262 host protocol is not implemented",
  );
  assert.equal(
    skipReason({ attrs: { flags: {}, features: [] }, contents: "assert.sameValue(1, 1);" }),
    null,
  );
  assert.equal(
    skipReason({
      attrs: { flags: {}, features: ["IsHTMLDDA"] },
      contents: "",
    }),
    "IsHTMLDDA is an HTML host feature, outside this runtime's scope",
  );
});
