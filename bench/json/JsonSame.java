// Benchmark (b), same algorithm as stdlib/alloc/json.unio.
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.LinkedHashMap;
import java.util.List;
import java.util.Map;

public class JsonSame {
    static int[] chars;
    static int position;

    static int peek() {
        return position < chars.length ? chars[position] : -1;
    }

    static int advance() {
        int c = peek();
        position++;
        return c;
    }

    static void skipSpace() {
        for (int c = peek(); c == ' ' || c == '\t' || c == '\n' || c == '\r'; c = peek()) advance();
    }

    static void expect(String word) {
        for (int c : word.codePoints().toArray()) {
            if (advance() != c) throw new IllegalStateException("expected " + word);
        }
    }

    static Object readValue() {
        skipSpace();
        switch (peek()) {
            case '{': return readObject();
            case '[': return readArray();
            case '"': return readText();
            case 't': expect("true"); return Boolean.TRUE;
            case 'f': expect("false"); return Boolean.FALSE;
            case 'n': expect("null"); return null;
            default: return readNumber();
        }
    }

    static Object readObject() {
        advance();
        Map<String, Object> fields = new LinkedHashMap<>();
        skipSpace();
        if (peek() == '}') { advance(); return fields; }
        for (;;) {
            skipSpace();
            String key = readText();
            skipSpace();
            expect(":");
            fields.put(key, readValue());
            skipSpace();
            if (advance() == '}') return fields;
        }
    }

    static Object readArray() {
        advance();
        List<Object> items = new ArrayList<>();
        skipSpace();
        if (peek() == ']') { advance(); return items; }
        for (;;) {
            items.add(readValue());
            skipSpace();
            if (advance() == ']') return items;
        }
    }

    static String readText() {
        advance();
        StringBuilder result = new StringBuilder();
        for (;;) {
            // A run of characters that need no escape is copied at once.
            int start = position;
            while (position < chars.length) {
                int c = chars[position];
                if (c == '"' || c == '\\' || c < ' ') break;
                position++;
            }
            if (position > start) result.append(new String(chars, start, position - start));
            int c = advance();
            if (c == '"') return result.toString();
            if (c == '\\') {
                int e = advance();
                switch (e) {
                    case 'u': {
                        StringBuilder hex = new StringBuilder();
                        for (int i = 0; i < 4; i++) hex.appendCodePoint(advance());
                        result.appendCodePoint(Integer.parseInt(hex.toString(), 16));
                        break;
                    }
                    case 'n': result.append('\n'); break;
                    case 'r': result.append('\r'); break;
                    case 't': result.append('\t'); break;
                    case 'b': result.append('\b'); break;
                    case 'f': result.append('\f'); break;
                    default: result.appendCodePoint(e);
                }
            } else {
                result.appendCodePoint(c);
            }
        }
    }

    static Object readNumber() {
        StringBuilder text = new StringBuilder();
        for (int c = peek(); "-+.eE".indexOf(c) >= 0 && c != -1 || (c >= '0' && c <= '9'); c = peek()) {
            text.appendCodePoint(advance());
        }
        String s = text.toString();
        if (s.contains(".") || s.contains("e") || s.contains("E")) return Double.parseDouble(s);
        return Long.parseLong(s);
    }

    static String escape(int c) {
        switch (c) {
            case '"': return "\\\"";
            case '\\': return "\\\\";
            case '\n': return "\\n";
            case '\r': return "\\r";
            case '\t': return "\\t";
            default:
                if (c < 0x20) return String.format("\\u%04x", c);
                return new String(Character.toChars(c));
        }
    }

    static String quote(String text) {
        String result = "\"";
        for (int c : text.codePoints().toArray()) result += escape(c);
        return result + "\"";
    }

    @SuppressWarnings("unchecked")
    static String writeValue(Object v) {
        if (v == null) return "null";
        if (v instanceof Boolean || v instanceof Long || v instanceof Double) return v.toString();
        if (v instanceof String) return quote((String) v);
        List<String> parts = new ArrayList<>();
        if (v instanceof List) {
            for (Object item : (List<Object>) v) parts.add(writeValue(item));
            return "[" + String.join(",", parts) + "]";
        }
        for (Map.Entry<String, Object> f : ((Map<String, Object>) v).entrySet()) {
            parts.add(quote(f.getKey()) + ":" + writeValue(f.getValue()));
        }
        return "{" + String.join(",", parts) + "}";
    }

    public static void main(String[] args) throws Exception {
        chars = Files.readString(Path.of(args[0]), StandardCharsets.UTF_8).codePoints().toArray();
        position = 0;
        System.out.println(writeValue(readValue()).getBytes(StandardCharsets.UTF_8).length);
    }
}
