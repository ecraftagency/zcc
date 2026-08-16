int printf(char *fmt, ...);
enum color { RED, GREEN = 5, BLUE };
int main() {
    enum color c;
    c = BLUE;
    printf("%d %d %d\n", RED, GREEN, c);
    return RED + GREEN + BLUE;
}
