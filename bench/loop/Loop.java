// Benchmark (d): naive recursion and an arithmetic loop.
public class Loop {
    static long fib(long n) {
        if (n < 2) return n;
        return fib(n - 1) + fib(n - 2);
    }

    public static void main(String[] args) {
        long total = 0;
        for (long i = 0; i < 10000000; i++) total = total + i * i % 7;
        System.out.println(fib(30) + " " + total);
    }
}
