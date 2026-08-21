/* Three pure-C89 bugs exposed by building git — regression tests:
   1. A global name enters scope at the END of its declarator, BEFORE the
      initializer (6.1.2.1) -> a self-referential static struct (the
      LIST_HEAD idiom of git/kernel).
   2. An identifier after the specifier in a declarator is ALWAYS a name,
      even when it collides with a global typedef-name (the report_fn
      parameter of reftable-fsck.h).
   3. The locals of a preceding function are irrelevant outside its body
      and must not leak into name resolution within a global initializer
      (the index_only parameter of check_local_mod versus the global
      index_only in builtin/rm.c). */
int printf(const char *, ...);

/* --- 1: self-referential initializer --- */
struct list_head { struct list_head *next, *prev; };
static struct list_head lst = { &lst, &lst };

/* --- 2: parameter name colliding with a typedef name --- */
typedef int report_fn(int);
static int twice(int x) { return 2 * x; }
static int call_cb(report_fn report_fn, int v) { return report_fn(v); }

/* --- 3: a preceding function's local must not leak into a later global initializer --- */
static int helper(int index_only) { return index_only + 1; }
static int index_only = 40;
static int *pio = &index_only;

int main(void) {
	printf("self=%d %d\n", lst.next == &lst, lst.prev == &lst);
	printf("cb=%d\n", call_cb(twice, 21));
	printf("leak=%d %d\n", *pio, helper(1));
	return 0;
}
