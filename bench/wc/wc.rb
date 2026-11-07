# Benchmark (a): lines, words, and UTF-8 bytes of a large text.
text = File.read(ARGV[0], encoding: "UTF-8")
puts "#{text.count("\n")} #{text.split.size} #{text.bytesize}"
