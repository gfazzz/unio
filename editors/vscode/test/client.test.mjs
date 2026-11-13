// Tests of the language server client of the extension (client.js): the framing of messages,
// and, when a `unio` built from this checkout is present, a session with `unio lsp`.
// Run from editors/vscode: npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import { existsSync, mkdtempSync, rmSync, writeFileSync, readFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, dirname } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { createRequire } from "node:module";

const require = createRequire(import.meta.url);
const { Client, Framer, frame } = require("../client.js");
const here = dirname(fileURLToPath(import.meta.url));
const repository = join(here, "..", "..", "..");

test("messages are framed by their length, also when split across chunks", () => {
    const received = [];
    const framer = new Framer((m) => received.push(m));
    const bytes = Buffer.concat([
        frame({ jsonrpc: "2.0", id: 1, result: "привет" }),
        frame({ jsonrpc: "2.0", method: "x", params: {} }),
    ]);
    for (let i = 0; i < bytes.length; i += 7) {
        framer.push(bytes.subarray(i, i + 7));
    }
    assert.deepEqual(received, [
        { jsonrpc: "2.0", id: 1, result: "привет" },
        { jsonrpc: "2.0", method: "x", params: {} },
    ]);
});

test("the package starts the language server for .unio files", () => {
    const manifest = JSON.parse(readFileSync(join(here, "..", "package.json"), "utf8"));
    assert.equal(manifest.main, "./extension.js");
    assert.ok(manifest.activationEvents.includes("onLanguage:unio"));
    assert.ok(existsSync(join(here, "..", "extension.js")));
    assert.equal(manifest.contributes.configuration.properties["unio.server.path"].default, "unio");
});

const executable = ["debug", "release"]
    .map((profile) => join(repository, "compiler", "target", profile, process.platform === "win32" ? "unio.exe" : "unio"))
    .find((path) => existsSync(path));

test("a session with unio lsp: diagnostics of a buffer, then shutdown", { skip: !executable && "no unio built in compiler/target" }, async () => {
    const dir = mkdtempSync(join(tmpdir(), "unio-vscode-"));
    try {
        writeFileSync(join(dir, "unio.toml"), '[package]\nname = "demo"\n');
        const file = join(dir, "main.unio");
        writeFileSync(file, 'public function main():\n    print("hi")\n');
        const env = { ...process.env };
        delete env.UNIO_WORKER_PROBE;
        const args = process.platform === "win32" ? ["--allow-weaker-sandbox"] : [];
        const client = new Client(executable, args, { cwd: dir, env });
        const published = new Promise((resolve) => client.on("textDocument/publishDiagnostics", resolve));
        const init = await client.request("initialize", { processId: null, rootUri: null, capabilities: {} });
        assert.equal(init.capabilities.renameProvider, true);
        assert.ok(init.capabilities.codeActionProvider);
        client.notify("initialized", {});
        const uri = pathToFileURL(file).href;
        client.notify("textDocument/didOpen", {
            textDocument: { uri, languageId: "unio", version: 1, text: "public function main():\n    print(missing)\n" },
        });
        const params = await published;
        assert.equal(params.diagnostics[0].code, "E0056");
        // A quick fix of the server is bound to the revision of its diagnostic (T-13).
        const diagnostic = params.diagnostics[0];
        assert.equal(diagnostic.data.revision, 1);
        const actions = await client.request("textDocument/codeAction", {
            textDocument: { uri },
            range: diagnostic.range,
            context: { diagnostics: [diagnostic] },
        });
        for (const action of actions) {
            assert.equal(action.edit.documentChanges[0].textDocument.version, 1);
        }
        assert.equal(await client.stop(), 0);
    } finally {
        rmSync(dir, { recursive: true, force: true });
    }
});
