// Benchmark (c): split every line on spaces, join with "-", replace a word, interpolate a record.
const fs = require("fs");
const text = fs.readFileSync(process.argv[2], "utf8");
const lines = text.split("\n");
if (lines.length > 0 && lines[lines.length - 1] === "") lines.pop();
let total = 0;
lines.forEach((line, index) => {
  const words = line.split(" ");
  const joined = words.join("-");
  const record = `${index}:${words.length}:${joined.replaceAll("the", "THE")}`;
  total += record.length;
});
console.log(total);
