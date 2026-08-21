int main() {
    char c;
    long l;
    c = 200; /* char is signed on Darwin -> -56; c/2 distinguishes ldrsb from ldrb */
    l = 60000;
    l = l + l;
    return c / 2 + l % 250 + sizeof(c) + sizeof(l) + sizeof(&c);
}
