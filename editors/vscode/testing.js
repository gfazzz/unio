// Test running for the extension (T-11): the tests of a .unio file are found in its text, run
// with `unio test --json`, and their results are matched back by name. This module does not use
// the VS Code API, so its tests run under plain Node.js; extension.js connects it to the
// Testing view of the editor.
"use strict";

const { spawn } = require("node:child_process");

/** The text of a Unio text literal without its quotes and escapes, as `unio test` names it. */
function description(literal) {
    let out = "";
    let depth = 0;
    for (let i = 0; i < literal.length; i++) {
        const c = literal[i];
        if (c === "\\" && i + 1 < literal.length) {
            const next = literal[++i];
            if (depth === 0) {
                out += { n: "\n", t: "\t", r: "\r", 0: "\0" }[next] || next;
            }
        } else if (c === "{") {
            if (depth === 0) {
                out += "{…}";
            }
            depth++;
        } else if (c === "}" && depth > 0) {
            depth--;
        } else if (depth === 0) {
            out += c;
        }
    }
    return out;
}

/** The `test` blocks of a module: their names, as `unio test` reports them, and lines (from 0). */
function testBlocks(text) {
    const found = [];
    const lines = text.split(/\r?\n/);
    const pattern = /^test\s+"((?:[^"\\]|\\.)*)"\s*:/;
    lines.forEach((line, i) => {
        const match = pattern.exec(line);
        if (match) {
            found.push({ name: description(match[1]), line: i, kind: "test" });
        }
    });
    return found;
}

function docText(line) {
    const trimmed = line.replace(/^\s+/, "");
    if (!trimmed.startsWith("##") || trimmed.startsWith("###")) {
        return null;
    }
    const rest = trimmed.slice(2);
    return rest.startsWith(" ") ? rest.slice(1) : rest;
}

function declaredName(line) {
    const words = line.split(/[^\p{L}\p{N}_]+/u).filter((w) => w);
    const skip = new Set(["public", "external", "function", "struct", "enum", "error", "interface", "unsafe"]);
    for (let i = 0; i < words.length; i++) {
        const w = words[i];
        if (skip.has(w)) {
            continue;
        }
        if (w === "extend") {
            return words[i + 1] ? `extend ${words[i + 1]}` : null;
        }
        if (["test", "import", "from", "module"].includes(w)) {
            return null;
        }
        return w;
    }
    return null;
}

/**
 * The examples of the documentation comments of a module, which `unio test` runs as tests named
 * `doc: DECLARATION (line N)` (compiler/unio/src/doctest.rs).
 */
function docExamples(text) {
    const lines = text.split(/\r?\n/);
    const found = [];
    let i = 0;
    while (i < lines.length) {
        if (docText(lines[i]) === null) {
            i++;
            continue;
        }
        const start = i;
        while (i < lines.length && docText(lines[i]) !== null) {
            i++;
        }
        const next = lines.slice(i).find((l) => l.trim() !== "");
        const declaration = (next && declaredName(next)) || "module";
        let inExamples = false;
        let current = null;
        for (let k = start; k < i; k++) {
            const t = docText(lines[k]);
            const heading = t.trim().toLowerCase();
            if (heading === "example:" || heading === "examples:") {
                inExamples = true;
                if (current) {
                    found.push(current);
                }
                current = null;
                continue;
            }
            if (inExamples && t.startsWith("    ")) {
                if (!current) {
                    current = { name: `doc: ${declaration} (line ${k + 1})`, line: k, kind: "doc" };
                }
            } else if (current) {
                found.push(current);
                current = null;
            }
        }
        if (current) {
            found.push(current);
        }
    }
    return found;
}

/** Every test `unio test` runs for a module: its test blocks and its documentation examples. */
function discover(text) {
    return [...testBlocks(text), ...docExamples(text)].sort((a, b) => a.line - b.line);
}

/** The arguments of `unio test` for a file, or for one test of it when `name` is given. */
function testArguments(file, name, options = {}) {
    const args = ["test", "--json", ...(options.extra || [])];
    if (options.seed !== undefined && options.seed !== null) {
        args.push("--seed", String(options.seed));
    }
    if (name) {
        args.push(name);
    }
    args.push(file);
    return args;
}

/** The records of `unio test --json`, by test name; a file that does not compile has name null. */
function parseResults(stdout) {
    const start = stdout.indexOf("[");
    if (start < 0) {
        return [];
    }
    let records;
    try {
        records = JSON.parse(stdout.slice(start));
    } catch {
        return [];
    }
    return records.map((r) => ({
        file: r.file,
        name: r.name,
        passed: r.status === "passed",
        message: r.message || "",
        output: r.output || "",
        duration: r.time_ms || 0,
        properties: r.properties || [],
    }));
}

/** The message of a failed test, with what replays a failing property (T-12). */
function failureMessage(result) {
    let text = result.message;
    for (const p of result.properties) {
        text += `\nproperty ${p.scheme}: seed ${p.seed}, case ${p.case_index}, sample seed ${p.sample_seed}, original ${p.original}, minimal ${p.minimal} after ${p.shrink_attempts} shrink attempts\nreplay: ${p.replay}`;
    }
    if (result.output) {
        text += `\n${result.output}`;
    }
    return text.trim();
}

/**
 * Runs `unio test --json` and resolves with the results, the exit code, and standard error.
 * The tests run in the sandbox of their package, as from the command line.
 */
function runTests(command, file, name, options = {}) {
    return new Promise((resolve) => {
        const child = spawn(command, testArguments(file, name, options), {
            cwd: options.cwd,
            env: options.env || process.env,
            stdio: ["ignore", "pipe", "pipe"],
        });
        let stdout = "";
        let stderr = "";
        child.stdout.on("data", (d) => (stdout += d));
        child.stderr.on("data", (d) => (stderr += d));
        if (options.token) {
            options.token.onCancellationRequested(() => child.kill());
        }
        child.on("error", (error) => resolve({ results: [], code: -1, stderr: String(error) }));
        child.on("close", (code) => resolve({ results: parseResults(stdout), code, stderr }));
    });
}

module.exports = { description, testBlocks, docExamples, discover, testArguments, parseResults, failureMessage, runTests };
