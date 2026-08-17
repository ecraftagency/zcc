/* 3 bug C89 thuần do build git phơi ra (2026-08-17) — regression:
   1. Tên global vào scope từ CUỐI declarator, TRƯỚC initializer (6.1.2.1)
      → static struct tự tham chiếu (LIST_HEAD của git/kernel).
   2. Ident sau specifier trong declarator LUÔN là tên, kể cả trùng
      typedef-name toàn cục (param report_fn của reftable-fsck.h).
   3. Locals của hàm trước là đồ thừa ngoài body — không được rò vào
      resolve tên trong initializer toàn cục (param index_only của
      check_local_mod vs global index_only trong builtin/rm.c). */
int printf(const char *, ...);

/* --- 1: initializer tự tham chiếu --- */
struct list_head { struct list_head *next, *prev; };
static struct list_head lst = { &lst, &lst };

/* --- 2: param name trùng typedef name --- */
typedef int report_fn(int);
static int twice(int x) { return 2 * x; }
static int call_cb(report_fn report_fn, int v) { return report_fn(v); }

/* --- 3: local hàm trước không rò vào ginit sau --- */
static int helper(int index_only) { return index_only + 1; }
static int index_only = 40;
static int *pio = &index_only;

int main(void) {
	printf("self=%d %d\n", lst.next == &lst, lst.prev == &lst);
	printf("cb=%d\n", call_cb(twice, 21));
	printf("leak=%d %d\n", *pio, helper(1));
	return 0;
}
