// Tests of test running in the extension (T-11, testing.js): finding the tests of a module, the
// arguments of `unio test`, reading its JSON, and, when a `unio` built from this checkout is
// present, running a file and matching each result to its test.
// Run from editors/vscode: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, rmSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const testing = require("../testing.js");
const here = dirname(fileURLToPath(import.meta.url));
const repository = join(here, "..", "..", "..");

const MODULE = `## Adds.
##
## Example:
##     assert add(2, 3) == 5
public function add(a as Int, b as Int) returns Int:
    return a + b

## Usage:
##     add A B
function other():
    pass

test "adds \\"small\\" numbers":
    assert add(1, 2) == 3

test "fails on {add(1, 1)}":
    assert add(1, 1) == 3

    test "not a test block: indented"
`;

test("tests are found in the text: test blocks and documentation examples", () => {
    const found = testing.discover(MODULE);
    assert.deepEqual(
        found.map((t) => [t.name, t.line, t.kind]),
        [
            ["doc: add (line 4)", 3, "doc"],
            ['adds "small" numbers', 12, "test"],
            ["fails on {…}", 15, "test"],
        ]
    );
});

test("every test block of the repository's tools is found", () => {
    const text = readFileSync(join(repository, "stdlib", "alloc", "test.unio"), "utf8");
    const blocks = testing.testBlocks(text);
    const count = (text.match(/^test "/gm) || []).length;
    assert.equal(blocks.length, count);
    assert.ok(count > 0);
});

test("the arguments of unio test, and its JSON read back", () => {
    assert.deepEqual(testing.testArguments("a.unio", null), ["test", "--json", "a.unio"]);
    assert.deepEqual(
        testing.testArguments("a.unio", "adds", { extra: ["--policy", "p.toml"], seed: 7 }),
        ["test", "--json", "--policy", "p.toml", "--seed", "7", "adds", "a.unio"]
    );
    const results = testing.parseResults(
        'warning\n[{"file": "a.unio", "name": "x", "status": "failed", "time_ms": 3, "output": "o", "message": "m", "properties": [{"scheme": "unio-property-v1", "seed": 3, "case_index": 1, "sample_seed": 9, "original": "607", "minimal": "500", "shrink_attempts": 2, "replay": "unio test --seed 3 \\"x\\" a.unio"}]}]'
    );
    assert.equal(results.length, 1);
    assert.equal(results[0].passed, false);
    assert.equal(results[0].duration, 3);
    const message = testing.failureMessage(results[0]);
    assert.match(message, /^m\nproperty unio-property-v1: seed 3/);
    assert.match(message, /replay: unio test --seed 3 "x" a.unio/);
    assert.deepEqual(testing.parseResults("not json"), []);
});

const executable = ["debug", "release"]
    .map((profile) => join(repository, "compiler", "target", profile, process.platform === "win32" ? "unio.exe" : "unio"))
    .find((path) => existsSync(path));

test("running a file reports each test by name", { skip: !executable && "no unio built in compiler/target" }, async () => {
    const dir = mkdtempSync(join(tmpdir(), "unio-vscode-tests-"));
    try {
        const file = join(dir, "lib.unio");
        writeFileSync(file, MODULE);
        const env = { ...process.env };
        delete env.UNIO_WORKER_PROBE;
        delete env.UNIO_POLICY;
        const done = await testing.runTests(executable, file, null, { cwd: dir, env });
        assert.equal(done.code, 1, done.stderr);
        const byName = Object.fromEntries(done.results.map((r) => [r.name, r.passed]));
        for (const t of testing.discover(MODULE)) {
            assert.ok(t.name in byName, `${t.name} in ${JSON.stringify(byName)}`);
        }
        assert.equal(byName['adds "small" numbers'], true);
        assert.equal(byName["fails on {…}"], false);
        assert.equal(byName["doc: add (line 4)"], true);
        // One test by its name.
        const one = await testing.runTests(executable, file, "doc: add", { cwd: dir, env });
        assert.deepEqual(one.results.map((r) => r.name), ["doc: add (line 4)"]);
        assert.equal(one.code, 0);
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
});
