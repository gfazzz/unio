# Benchmark (b), the algorithm of stdlib/alloc/json.unio step by step: a reader over the list of
# characters that tracks line, column, and depth, checks duplicate keys and control characters,
# builds numbers as text; a writer that quotes text character by character.
import sys

DECIMAL_DIGITS = "0123456789"
HEX_DIGITS = "0123456789abcdef"


class ParseError(Exception):
    pass


class Reader:
    __slots__ = ("text", "characters", "position", "line", "column", "depth", "max_depth")

    def __init__(self, text, characters, max_depth):
        self.text = text
        self.characters = characters
        self.position = 0
        self.line = 1
        self.column = 1
        self.depth = 0
        self.max_depth = max_depth


def parse(text, max_depth=128, max_bytes=16777216):
    if max_depth <= 0 or max_bytes <= 0 or len(text.encode()) > max_bytes:
        raise ParseError("invalid or exceeded JSON size limit")
    reader = Reader(text, list(text), max_depth)
    value = read_value(reader)
    skip_space(reader)
    if not at_end(reader):
        raise error_at(reader, "unexpected text after the value")
    return value


def at_end(reader):
    return reader.position >= len(reader.characters)


def peek(reader):
    return reader.characters[reader.position] if reader.position < len(reader.characters) else ""


def advance(reader):
    character = peek(reader)
    reader.position = reader.position + 1
    if character == "\n":
        reader.line = reader.line + 1
        reader.column = 1
    else:
        reader.column = reader.column + 1
    return character


def error_at(reader, message):
    return ParseError(f"{reader.line}:{reader.column}: {message}")


def skip_space(reader):
    while peek(reader) in [" ", "\t", "\n", "\r"]:
        advance(reader)


def is_digit(character):
    return len(character) == 1 and character in DECIMAL_DIGITS


def expect(reader, word):
    for character in word:
        if advance(reader) != character:
            raise error_at(reader, f'expected "{word}"')


def read_value(reader):
    if reader.depth >= reader.max_depth:
        raise error_at(reader, "JSON depth limit exceeded")
    reader.depth = reader.depth + 1
    value = read_nested_value(reader)
    reader.depth = reader.depth - 1
    return value


def read_nested_value(reader):
    skip_space(reader)
    character = peek(reader)
    if character == "{":
        return read_object(reader)
    elif character == "[":
        return read_array(reader)
    elif character == '"':
        return read_text(reader)
    elif character == "t":
        expect(reader, "true")
        return True
    elif character == "f":
        expect(reader, "false")
        return False
    elif character == "n":
        expect(reader, "null")
        return None
    elif character == "-" or is_digit(character):
        return read_number(reader)
    elif at_end(reader):
        raise error_at(reader, "unexpected end of the document")
    raise error_at(reader, f'unexpected character "{character}"')


def read_object(reader):
    advance(reader)
    fields = {}
    skip_space(reader)
    if peek(reader) == "}":
        advance(reader)
        return fields
    while True:
        skip_space(reader)
        if peek(reader) != '"':
            raise error_at(reader, "expected a key in quotes")
        key = read_text(reader)
        if key in fields:
            raise error_at(reader, f"duplicate object key: {key}")
        skip_space(reader)
        expect(reader, ":")
        fields[key] = read_value(reader)
        skip_space(reader)
        separator = advance(reader)
        if separator == "}":
            return fields
        if separator != ",":
            raise error_at(reader, 'expected "," or "}" in an object')


def read_array(reader):
    advance(reader)
    items = []
    skip_space(reader)
    if peek(reader) == "]":
        advance(reader)
        return items
    while True:
        items.append(read_value(reader))
        skip_space(reader)
        separator = advance(reader)
        if separator == "]":
            return items
        if separator != ",":
            raise error_at(reader, 'expected "," or "]" in an array')


def read_text(reader):
    advance(reader)
    result = ""
    while True:
        # A run of characters that need no escape, which stays on one line, is copied at once.
        start = reader.position
        while reader.position < len(reader.characters):
            character = reader.characters[reader.position]
            if character == '"' or character == "\\" or character < " ":
                break
            reader.position = reader.position + 1
        if reader.position > start:
            reader.column = reader.column + reader.position - start
            result = result + reader.text[start:reader.position]
        if at_end(reader):
            raise error_at(reader, "unterminated text")
        character = advance(reader)
        if character == '"':
            return result
        if character == "\\":
            result = result + read_escape(reader)
        else:
            raise error_at(reader, "a control character must be escaped in text")


ESCAPES = {'"': '"', "\\": "\\", "/": "/", "b": "\b", "f": "\f", "n": "\n", "r": "\r", "t": "\t"}


def read_escape(reader):
    character = advance(reader)
    if character in ESCAPES:
        return ESCAPES[character]
    if character == "u":
        return read_unicode(reader)
    raise error_at(reader, f'invalid escape "\\{character}"')


def read_unicode(reader):
    first = read_hex4(reader)
    code = first
    if 0xD800 <= first <= 0xDBFF:
        expect(reader, "\\u")
        second = read_hex4(reader)
        if second < 0xDC00 or second > 0xDFFF:
            raise error_at(reader, "invalid surrogate pair")
        code = 0x10000 + (first - 0xD800) * 0x400 + (second - 0xDC00)
    if 0xD800 <= code <= 0xDFFF or code > 0x10FFFF:
        raise error_at(reader, f"invalid code point {code}")
    return chr(code)


def read_hex4(reader):
    value = 0
    for _ in range(0, 4):
        character = advance(reader)
        if len(character) != 1:
            raise error_at(reader, "expected four hexadecimal digits")
        digit = HEX_DIGITS.find(character.lower())
        if digit < 0:
            raise error_at(reader, "expected four hexadecimal digits")
        value = value * 16 + digit
    return value


def read_number(reader):
    text = ""
    is_float = False
    if peek(reader) == "-":
        text = text + advance(reader)
    if not is_digit(peek(reader)):
        raise error_at(reader, "expected a digit")
    if peek(reader) == "0":
        text = text + advance(reader)
    else:
        while is_digit(peek(reader)):
            text = text + advance(reader)
    if peek(reader) == ".":
        is_float = True
        text = text + advance(reader)
        if not is_digit(peek(reader)):
            raise error_at(reader, 'expected a digit after "."')
        while is_digit(peek(reader)):
            text = text + advance(reader)
    if peek(reader) in ["e", "E"]:
        is_float = True
        text = text + advance(reader)
        if peek(reader) in ["+", "-"]:
            text = text + advance(reader)
        if not is_digit(peek(reader)):
            raise error_at(reader, "expected a digit in the exponent")
        while is_digit(peek(reader)):
            text = text + advance(reader)
    if not is_float:
        number = int(text)
        if -(2**63) <= number < 2**63:
            return number
    return float(text)


def write_value(value, pretty, depth):
    if value is None:
        return "null"
    if value is True:
        return "true"
    if value is False:
        return "false"
    if isinstance(value, int):
        return str(value)
    if isinstance(value, float):
        return repr(value)
    if isinstance(value, str):
        return quote(value)
    if isinstance(value, list):
        if not value:
            return "[]"
        parts = [write_value(item, pretty, depth + 1) for item in value]
        return wrap("[", parts, "]", pretty, depth)
    if not value:
        return "{}"
    colon = ": " if pretty else ":"
    parts = [quote(key) + colon + write_value(field, pretty, depth + 1) for key, field in value.items()]
    return wrap("{", parts, "}", pretty, depth)


def wrap(open_, parts, close, pretty, depth):
    if not pretty:
        return open_ + ",".join(parts) + close
    inner = "    " * (depth + 1)
    return open_ + "\n" + inner + (",\n" + inner).join(parts) + "\n" + "    " * depth + close


def quote(text):
    result = '"'
    start = 0
    index = 0
    for character in text:
        # Runs of characters that need no escape are copied at once.
        if character == '"' or character == "\\" or character < " ":
            result = result + text[start:index] + escape(character)
            start = index + 1
        index = index + 1
    return result + text[start:len(text)] + '"'


def escape(character):
    if character == '"':
        return '\\"'
    if character == "\\":
        return "\\\\"
    if character == "\n":
        return "\\n"
    if character == "\r":
        return "\\r"
    if character == "\t":
        return "\\t"
    if character < " ":
        return "\\u" + hex4(ord(character))
    return character


def hex4(code):
    digits = ""
    rest = code
    for _ in range(0, 4):
        digits = HEX_DIGITS[rest % 16 : rest % 16 + 1] + digits
        rest = rest // 16
    return digits


with open(sys.argv[1], encoding="utf-8") as f:
    text = f.read()
print(len(write_value(parse(text), False, 0).encode()))
