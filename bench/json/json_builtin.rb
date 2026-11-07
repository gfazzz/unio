# Benchmark (b), the language's own JSON library.
require "json"
value = JSON.parse(File.read(ARGV[0], encoding: "UTF-8"))
puts JSON.generate(value).bytesize
