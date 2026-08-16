int main() {
    char c;
    long l;
    c = 200; /* char signed trên Darwin -> -56; c/2 phân biệt ldrsb với ldrb */
    l = 60000;
    l = l + l;
    return c / 2 + l % 250 + sizeof(c) + sizeof(l) + sizeof(&c);
}
