// Benchmark (a): lines, words, and UTF-8 bytes of a large text.
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

public class Wc {
    public static void main(String[] args) throws Exception {
        String text = Files.readString(Path.of(args[0]));
        long lines = text.chars().filter(c -> c == '\n').count();
        String trimmed = text.strip();
        int words = trimmed.isEmpty() ? 0 : trimmed.split("\\s+").length;
        int bytes = text.getBytes(StandardCharsets.UTF_8).length;
        System.out.println(lines + " " + words + " " + bytes);
    }
}
