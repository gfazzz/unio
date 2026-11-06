// Tests of the TextMate grammar with the engine that VS Code and Cursor use.
// Run from editors/vscode: npm install && npm test

import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import vsctm from "vscode-textmate";
import oniguruma from "vscode-oniguruma";

const here = dirname(fileURLToPath(import.meta.url));
const extension = join(here, "..");
const repository = join(extension, "..", "..");
const require = createRequire(import.meta.url);

const wasm = readFileSync(require.resolve("vscode-oniguruma/release/onig.wasm")).buffer;
await oniguruma.loadWASM(wasm);

const registry = new vsctm.Registry({
    onigLib: Promise.resolve({
        createOnigScanner: (patterns) => new oniguruma.OnigScanner(patterns),
        createOnigString: (text) => new oniguruma.OnigString(text),
    }),
    loadGrammar: async (scopeName) => {
        const files = {
            "source.unio": "syntaxes/unio.tmLanguage.json",
            "markdown.unio.codeblock": "syntaxes/unio.markdown.tmLanguage.json",
        };
        const file = files[scopeName];
        if (!file) return null;
        const text = readFileSync(join(extension, file), "utf8");
        return vsctm.parseRawGrammar(text, join(extension, file));
    },
});

const grammar = await registry.loadGrammar("source.unio");

// Tokens of a source text as [text, scopes] pairs, line by line.
function tokenize(source) {
    let state = vsctm.INITIAL;
    const result = [];
    for (const line of source.split("\n")) {
        const { tokens, ruleStack } = grammar.tokenizeLine(line, state);
        for (const token of tokens) {
            result.push([line.slice(token.startIndex, token.endIndex), token.scopes]);
        }
        state = ruleStack;
    }
    return result;
}

// The scopes of the first token with exactly this text, starting at an optional occurrence.
function scopesOf(source, text, occurrence = 0) {
    const found = tokenize(source).filter(([t]) => t === text);
    assert.ok(found.length > occurrence, `no token "${text}" in:\n${source}`);
    return found[occurrence][1];
}

function hasScope(source, text, scope, occurrence = 0) {
    const scopes = scopesOf(source, text, occurrence);
    assert.ok(scopes.some((s) => s.startsWith(scope)), `"${text}" has ${scopes.join(" ")}, expected ${scope}`);
}

function lacksScope(source, text, scope, occurrence = 0) {
    const scopes = scopesOf(source, text, occurrence);
    assert.ok(!scopes.some((s) => s.startsWith(scope)), `"${text}" has ${scopes.join(" ")}, did not expect ${scope}`);
}

test("declarations and their names", () => {
    const source = "public function greet(name as Text) returns Text:\n    return \"Hello\"";
    hasScope(source, "public", "storage.modifier");
    hasScope(source, "function", "storage.type.function");
    hasScope(source, "greet", "entity.name.function");
    hasScope(source, "as", "keyword.other");
    hasScope(source, "Text", "support.type");
    hasScope(source, "returns", "keyword.other");
    hasScope(source, "return", "keyword.control");
});

test("types, structs, enums and interfaces", () => {
    const source = "struct Account:\n    public var balance as Int = 0\nenum Shape:\n    Circle(radius as Float)\ninterface Display:\n    function to_text(self) returns Text";
    hasScope(source, "struct", "storage.type");
    hasScope(source, "Account", "entity.name.type");
    hasScope(source, "var", "storage.modifier");
    hasScope(source, "Shape", "entity.name.type");
    hasScope(source, "Circle", "entity.name.type");
    hasScope(source, "Display", "entity.name.type");
    hasScope(source, "self", "variable.language.self");
});

test("control flow, errors and resources", () => {
    const source = "if a and not b:\n    pass\nfor item in items:\n    continue\nlet text = try read(path) on IoError as e:\n    fail Missing(path = path)\nusing file = try open(path):\n    match value:\n        case some(x):\n            break";
    for (const word of ["if", "for", "in", "try", "on", "fail", "using", "match", "case", "pass", "continue", "break"]) {
        hasScope(source, word, "keyword.control");
    }
    hasScope(source, "and", "keyword.operator.logical");
    hasScope(source, "not", "keyword.operator.logical");
    hasScope(source, "let", "storage.type");
    hasScope(source, "IoError", "entity.name.type");
    hasScope(source, "some", "support.function");
});

test("text with escapes and interpolation", () => {
    const source = "let s = \"Hello, {user.name}! \\n \\{literal\\}\"";
    hasScope(source, "\"", "punctuation.definition.string.begin");
    hasScope(source, "Hello, ", "string.quoted.double");
    hasScope(source, "{", "punctuation.section.interpolation.begin");
    hasScope(source, "user", "meta.interpolation");
    lacksScope(source, "user", "string.quoted.double.unio");
    hasScope(source, "\\n", "constant.character.escape");
    hasScope(source, "\\{", "constant.character.escape");
});

test("nested text inside an interpolation", () => {
    const source = "print(\"{join(parts, \", \")} done\")";
    hasScope(source, "join", "meta.interpolation");
    hasScope(source, " done", "string.quoted.double");
});

test("numbers, constants and comments", () => {
    const source = "## Documentation\nlet n = 1_000 + 0xFF + 0b1010 + 2.5e-3  # comment\nlet ok = true or false";
    hasScope(source, "## Documentation", "comment.line.documentation");
    hasScope(source, "1_000", "constant.numeric");
    hasScope(source, "0xFF", "constant.numeric");
    hasScope(source, "0b1010", "constant.numeric");
    hasScope(source, "2.5e-3", "constant.numeric");
    hasScope(source, "# comment", "comment.line.number-sign");
    hasScope(source, "true", "constant.language");
    hasScope(source, "or", "keyword.operator.logical");
});

test("contextual keywords are keywords only in their positions", () => {
    const source = "public error Missing:\n    path as Path\ntest \"it works\":\n    let error = 1\nunsafe module\nexternal function now() returns Int\nfunction f(named port as Int, test as Int):\n    pass";
    hasScope(source, "error", "storage.type", 0);
    hasScope(source, "Missing", "entity.name.type");
    hasScope(source, "test", "storage.type", 0);
    lacksScope(source, "error", "storage.type", 1);
    lacksScope(source, "test", "storage.type", 1);
    hasScope(source, "unsafe", "storage.modifier");
    hasScope(source, "module", "storage.modifier");
    hasScope(source, "external", "storage.modifier");
    hasScope(source, "named", "storage.modifier");
});

test("one-line branches, pattern tests and lambdas without types", () => {
    const source = "if fields.get(key) is some(child): return child\nlet ids = tasks.map(function (task): task.id)";
    hasScope(source, "is", "keyword.operator.logical");
    hasScope(source, "some", "support.function");
    hasScope(source, "return", "keyword.control");
    hasScope(source, "function", "storage.type.function");
    hasScope(source, "task", "variable.other", 0);
});

test("calls and methods", () => {
    const source = "let x = items.map(change)\nlet y = Account.open(\"Anna\")\nprint(x)";
    hasScope(source, "map", "entity.name.function.call");
    hasScope(source, "open", "entity.name.function.call");
    hasScope(source, "print", "entity.name.function.call");
    hasScope(source, "Account", "entity.name.type");
});

test("every keyword of spec/grammar.ebnf is highlighted", () => {
    const ebnf = readFileSync(join(repository, "spec", "grammar.ebnf"), "utf8");
    const between = (start, end) => {
        const i = ebnf.indexOf(start);
        const j = ebnf.indexOf(end, i);
        return ebnf.slice(i + start.length, j).split(/\s+/).filter(Boolean);
    };
    const keywords = between("requires an RFC):", "Contextual keywords");
    const reserved = between("(using them is E0032):", "Punctuation");
    assert.ok(keywords.length > 30, "keyword list not found in the grammar");
    const missing = [];
    for (const word of [...keywords, ...reserved]) {
        const scopes = scopesOf(`${word} `, word);
        const keywordLike = /^(keyword|storage|constant\.language|variable\.language)\./;
        if (!scopes.some((s) => keywordLike.test(s))) missing.push(word);
    }
    assert.deepEqual(missing, [], `keywords without highlighting: ${missing.join(", ")}`);
});

test("every .unio file of the repository is tokenized without invalid scopes", () => {
    const files = [];
    const walk = (dir) => {
        for (const name of readdirSync(dir)) {
            if (name === "node_modules" || name === ".git" || name === "target") continue;
            const path = join(dir, name);
            if (statSync(path).isDirectory()) walk(path);
            else if (name.endsWith(".unio") && !path.includes(join("spec", "tests", "invalid"))) files.push(path);
        }
    };
    walk(repository);
    assert.ok(files.length > 30);
    for (const file of files) {
        for (const [text, scopes] of tokenize(readFileSync(file, "utf8"))) {
            assert.ok(!scopes.some((s) => s.startsWith("invalid")), `${file}: "${text}" is ${scopes.join(" ")}`);
        }
    }
});

test("unio blocks in Markdown are highlighted as Unio", async () => {
    const markdown = await registry.loadGrammarWithConfiguration("markdown.unio.codeblock", 0, {});
    assert.ok(markdown, "the Markdown injection grammar does not load");
    const text = readFileSync(join(extension, "syntaxes", "unio.markdown.tmLanguage.json"), "utf8");
    const raw = JSON.parse(text);
    assert.equal(raw.injectionSelector, "L:text.html.markdown");
    assert.match(JSON.stringify(raw), /source\.unio/);
});

test("the language has a file icon for light and dark themes", () => {
    const manifest = JSON.parse(readFileSync(join(extension, "package.json"), "utf8"));
    const icon = manifest.contributes.languages[0].icon;
    for (const theme of ["light", "dark"]) {
        const svg = readFileSync(join(extension, icon[theme]), "utf8");
        assert.match(svg, /^<svg[^>]*viewBox="0 0 16 16"/);
    }
});
