# Benchmark (c): split every line on spaces, join with "-", replace a word, interpolate a record.
text = File.read(ARGV[0], encoding: "UTF-8")
total = 0
text.each_line(chomp: true).with_index do |line, index|
  words = line.split(" ", -1)
  joined = words.join("-")
  record = "#{index}:#{words.length}:#{joined.gsub("the", "THE")}"
  total += record.length
end
puts total
