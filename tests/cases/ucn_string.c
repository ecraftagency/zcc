/* C99 6.4.3 - universal character names in string literals.
 *
 * WHY THIS CASE EXISTS. zcc had no \\u arm in its escape handler at all, so a
 * UCN fell through to the UNDEFINED-ESCAPE identity rule (C89 3.1.3.4, which is
 * right for \\j), the backslash was dropped, and the escape for OHM SIGN became
 * the five characters u2126. Silent wrong answers, and nothing caught them:
 * torture, c-testsuite, opt-parity and both fuzzers were green, and so was every
 * benchmark - a differential comparison covers only what its workload executes,
 * and nothing in the corpus wrote a UCN. oniguruma's own 1,516-case suite found
 * it, 5 failures.
 *
 * EVERY LITERAL BELOW IS AN ESCAPE, NOT A GLYPH. Two earlier drafts of this case
 * pasted the actual characters, which tests that UTF-8 SOURCE BYTES survive to
 * the output - a different property, and one that already worked - so the case
 * caught only the single line that did use an escape. That is the whole trap:
 * the two spellings look identical in a terminal and test different things.
 *
 * The referee is the system cc at -std=c99, so this ASSERTS the encoding rather
 * than describing it: a UCN in a narrow string is the UTF-8 encoding of the code
 * point, one to four bytes, never one.
 */
int printf(const char *, ...);

static void bytes(const char *tag, const char *s) {
    int i;
    printf("%s:", tag);
    for (i = 0; s[i]; i++) printf(" %02X", (unsigned char)s[i]);
    printf(" len=%d\n", i);
}

int main(void) {
    /* every width the encoder has: two, three and four bytes */
    bytes("00e9",   "\u00e9");
    bytes("07ff",   "\u07ff");   /* last two-byte code point   */
    bytes("0800",   "\u0800");   /* first three-byte code point */
    bytes("2126",   "\u2126");   /* OHM SIGN                    */
    bytes("fb00",   "\ufb00");   /* LATIN SMALL LIGATURE FF     */
    bytes("ffff",   "\uffff");   /* last three-byte code point  */
    bytes("10000",  "\U00010000");   /* first four-byte code point */
    bytes("1f600",  "\U0001F600");   /* GRINNING FACE              */
    bytes("10ffff", "\U0010FFFF");   /* last code point            */

    /* the \U form spelling a value the \u form could also have spelled, and
       lower-case hex digits in the escape */
    bytes("Ushort", "\U00002126");
    bytes("lower",  "\u00fc");

    /* the three code points below A0 a UCN may still name (6.4.3p2) */
    bytes("dollar", "\u0024");
    bytes("at",     "\u0040");
    bytes("grave",  "\u0060");

    /* adjacent to ordinary text and to other escapes, because the defect was
       that the backslash was eaten and the rest ran on as plain characters */
    bytes("mixed",  "a\u2126b");
    bytes("twice",  "\u2126\u2126");
    bytes("esc",    "\t\u00e9\n");
    bytes("digit",  "\u00e99");   /* a digit after it must not be eaten */
    bytes("concat", "\u00e9" "x");

    /* a wide string keeps the code point rather than its encoding */
    printf("wide: %d %d %d\n", (int)L"\u2126"[0],
           (int)L"\U0001F600"[0], (int)L"a\u00e9"[1]);
    return 0;
}
