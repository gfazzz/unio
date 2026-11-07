// Benchmark (b), same algorithm as stdlib/alloc/json.unio.
const fs = require("fs");

class Reader {
  constructor(text) {
    this.chars = Array.from(text);
    this.position = 0;
  }
  peek() {
    return this.position < this.chars.length ? this.chars[this.position] : "";
  }
  advance() {
    const c = this.peek();
    this.position += 1;
    return c;
  }
}

function skipSpace(r) {
  while (r.peek() === " " || r.peek() === "\t" || r.peek() === "\n" || r.peek() === "\r") r.advance();
}

function expect(r, word) {
  for (const c of word) if (r.advance() !== c) throw new Error("expected " + word);
}

function readValue(r) {
  skipSpace(r);
  const c = r.peek();
  if (c === "{") return readObject(r);
  if (c === "[") return readArray(r);
  if (c === '"') return readText(r);
  if (c === "t") { expect(r, "true"); return true; }
  if (c === "f") { expect(r, "false"); return false; }
  if (c === "n") { expect(r, "null"); return null; }
  return readNumber(r);
}

function readObject(r) {
  r.advance();
  const fields = new Map();
  skipSpace(r);
  if (r.peek() === "}") { r.advance(); return fields; }
  for (;;) {
    skipSpace(r);
    const key = readText(r);
    skipSpace(r);
    expect(r, ":");
    fields.set(key, readValue(r));
    skipSpace(r);
    if (r.advance() === "}") return fields;
  }
}

function readArray(r) {
  r.advance();
  const items = [];
  skipSpace(r);
  if (r.peek() === "]") { r.advance(); return items; }
  for (;;) {
    items.push(readValue(r));
    skipSpace(r);
    if (r.advance() === "]") return items;
  }
}

const ESCAPES = { '"': '"', "\\": "\\", "/": "/", n: "\n", r: "\r", t: "\t", b: "\b", f: "\f" };

function readText(r) {
  r.advance();
  let result = "";
  for (;;) {
    // A run of characters that need no escape is copied at once.
    const start = r.position;
    while (r.position < r.chars.length) {
      const c = r.chars[r.position];
      if (c === '"' || c === "\\" || c < " ") break;
      r.position += 1;
    }
    if (r.position > start) result += r.chars.slice(start, r.position).join("");
    const c = r.advance();
    if (c === '"') return result;
    if (c === "\\") {
      const e = r.advance();
      if (e === "u") {
        let hex = "";
        for (let i = 0; i < 4; i++) hex += r.advance();
        result += String.fromCodePoint(parseInt(hex, 16));
      } else {
        result += ESCAPES[e];
      }
    } else {
      result += c;
    }
  }
}

function readNumber(r) {
  let text = "";
  while ("-+.eE".includes(r.peek()) && r.peek() !== "" || (r.peek() >= "0" && r.peek() <= "9" && r.peek() !== "")) {
    text += r.advance();
  }
  return /[.eE]/.test(text) ? parseFloat(text) : BigInt(text);
}

function escape(c) {
  if (c === '"') return '\\"';
  if (c === "\\") return "\\\\";
  if (c === "\n") return "\\n";
  if (c === "\r") return "\\r";
  if (c === "\t") return "\\t";
  const code = c.codePointAt(0);
  if (code < 0x20) return "\\u" + code.toString(16).padStart(4, "0");
  return c;
}

function quote(text) {
  const chars = Array.from(text);
  let result = '"';
  let start = 0;
  for (let index = 0; index < chars.length; index++) {
    // Runs of characters that need no escape are copied at once.
    const c = chars[index];
    if (c === '"' || c === "\\" || c < " ") {
      result += chars.slice(start, index).join("") + escape(c);
      start = index + 1;
    }
  }
  return result + chars.slice(start).join("") + '"';
}

function writeValue(v) {
  if (v === null) return "null";
  if (v === true) return "true";
  if (v === false) return "false";
  if (typeof v === "bigint" || typeof v === "number") return v.toString();
  if (typeof v === "string") return quote(v);
  if (Array.isArray(v)) return "[" + v.map(writeValue).join(",") + "]";
  const parts = [];
  for (const [k, f] of v) parts.push(quote(k) + ":" + writeValue(f));
  return "{" + parts.join(",") + "}";
}

const value = readValue(new Reader(fs.readFileSync(process.argv[2], "utf8")));
console.log(Buffer.byteLength(writeValue(value), "utf8"));
