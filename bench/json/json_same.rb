# Benchmark (b), same algorithm as stdlib/alloc/json.unio.
class Reader
  attr_accessor :chars, :position
  def initialize(text)
    @chars = text.chars
    @position = 0
  end
  def peek
    @position < @chars.length ? @chars[@position] : ""
  end
  def advance
    c = peek
    @position += 1
    c
  end
end

def skip_space(r)
  r.advance while [" ", "\t", "\n", "\r"].include?(r.peek)
end

def expect(r, word)
  word.each_char { |c| raise "expected #{word}" if r.advance != c }
end

def read_value(r)
  skip_space(r)
  c = r.peek
  return read_object(r) if c == "{"
  return read_array(r) if c == "["
  return read_text(r) if c == '"'
  if c == "t"
    expect(r, "true")
    return true
  end
  if c == "f"
    expect(r, "false")
    return false
  end
  if c == "n"
    expect(r, "null")
    return nil
  end
  read_number(r)
end

def read_object(r)
  r.advance
  fields = {}
  skip_space(r)
  if r.peek == "}"
    r.advance
    return fields
  end
  loop do
    skip_space(r)
    key = read_text(r)
    skip_space(r)
    expect(r, ":")
    fields[key] = read_value(r)
    skip_space(r)
    return fields if r.advance == "}"
  end
end

def read_array(r)
  r.advance
  items = []
  skip_space(r)
  if r.peek == "]"
    r.advance
    return items
  end
  loop do
    items << read_value(r)
    skip_space(r)
    return items if r.advance == "]"
  end
end

ESCAPES = { '"' => '"', "\\" => "\\", "/" => "/", "n" => "\n", "r" => "\r", "t" => "\t", "b" => "\b", "f" => "\f" }

def read_text(r)
  r.advance
  result = +""
  loop do
    # A run of characters that need no escape is copied at once.
    start = r.position
    r.position += 1 while r.position < r.chars.length && (c = r.chars[r.position]) != '"' && c != "\\" && c >= " "
    result << r.chars[start...r.position].join if r.position > start
    c = r.advance
    return result if c == '"'
    if c == "\\"
      e = r.advance
      if e == "u"
        result << Array.new(4) { r.advance }.join.to_i(16).chr(Encoding::UTF_8)
      else
        result << ESCAPES[e]
      end
    else
      result << c
    end
  end
end

def read_number(r)
  text = +""
  text << r.advance while ["-", "+", ".", "e", "E"].include?(r.peek) || r.peek.match?(/\A[0-9]\z/)
  text.match?(/[.eE]/) ? text.to_f : text.to_i
end

def escape(c)
  case c
  when '"' then '\\"'
  when "\\" then "\\\\"
  when "\n" then "\\n"
  when "\r" then "\\r"
  when "\t" then "\\t"
  else c.ord < 0x20 ? format("\\u%04x", c.ord) : c
  end
end

def quote(text)
  result = +'"'
  chars = text.chars
  start = 0
  chars.each_with_index do |c, index|
    # Runs of characters that need no escape are copied at once.
    next unless c == '"' || c == "\\" || c < " "
    result << chars[start...index].join << escape(c)
    start = index + 1
  end
  result << chars[start..].join << '"'
end

def write_value(v)
  case v
  when nil then "null"
  when true then "true"
  when false then "false"
  when Integer then v.to_s
  when String then quote(v)
  when Array then "[" + v.map { |item| write_value(item) }.join(",") + "]"
  else "{" + v.map { |k, f| quote(k) + ":" + write_value(f) }.join(",") + "}"
  end
end

value = read_value(Reader.new(File.read(ARGV[0], encoding: "UTF-8")))
puts write_value(value).bytesize
