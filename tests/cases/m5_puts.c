int puts(char *s);
int strlen(char *s);
int main() {
    char *msg;
    msg = "puts works";
    puts(msg);
    return strlen(msg);
}
