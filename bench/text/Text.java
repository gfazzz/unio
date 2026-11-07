// Benchmark (c): split every line on spaces, join with "-", replace a word, interpolate a record.
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;

public class Text {
    public static void main(String[] args) throws Exception {
        String text = Files.readString(Path.of(args[0]), StandardCharsets.UTF_8);
        String[] lines = text.split("\n");
        long total = 0;
        for (int index = 0; index < lines.length; index++) {
            String[] words = lines[index].split(" ", -1);
            String joined = String.join("-", words);
            String record = index + ":" + words.length + ":" + joined.replace("the", "THE");
            total += record.codePointCount(0, record.length());
        }
        System.out.println(total);
    }
}
