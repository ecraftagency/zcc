/* C99 6.3.1.2: chuyển sang _Bool = (value != 0) ? 1 : 0 — KHÔNG thu hẹp modular.
   Regression: const-fold cast→_Bool từng làm `v as u8` (size 1 + unsigned) nên
   (_Bool)0x100 ra 0 (low byte 0). yarpgen seed 59/62/96/145/178/187/229/262/275. */
int printf(char *fmt, ...);

int id(int x) { return x; } /* chặn fold: ép đường runtime */

int main() {
    /* hằng: đi qua const-fold parser */
    printf("%d %d %d %d\n", (_Bool)256, (_Bool)0x666B00, (_Bool)0x10000, (_Bool)0);
    /* trong điều kiện if (đúng ca yarpgen) */
    printf("%d %d\n", ((_Bool)6712832) ? 1 : 0, ((_Bool)0) ? 1 : 0);
    /* runtime: đi qua codegen ext(BOOL) */
    printf("%d %d %d\n", (_Bool)id(256), (_Bool)id(0x10000), (_Bool)id(0));
    /* long với low-32 = 0 */
    printf("%d %d\n", (_Bool)0x100000000L, (_Bool)id(0));
    return 0;
}
