/* K&R 4.10: recursive qsort on int + ex4-12 recursive itoa + static buffer */
#include <stdio.h>
void swap(int v[], int i, int j) {
    int temp = v[i];
    v[i] = v[j];
    v[j] = temp;
}
void my_qsort(int v[], int left, int right) {
    int i, last;
    if (left >= right) return;
    swap(v, left, (left + right) / 2);
    last = left;
    for (i = left + 1; i <= right; i++)
        if (v[i] < v[left]) swap(v, ++last, i);
    swap(v, left, last);
    my_qsort(v, left, last - 1);
    my_qsort(v, last + 1, right);
}
void itoa_rec(int n, char s[]) {
    static int i;
    if (n / 10)
        itoa_rec(n / 10, s);
    else {
        i = 0;
        if (n < 0) s[i++] = '-';
    }
    s[i++] = (n < 0 ? -(n % 10) : n % 10) + '0';
    s[i] = '\0';
}
int main(void) {
    int v[12] = {9, -3, 5, 0, 42, -17, 8, 8, 1, 100, -3, 7};
    int i;
    char buf[16];
    my_qsort(v, 0, 11);
    for (i = 0; i < 12; i++) printf("%d ", v[i]);
    printf("\n");
    itoa_rec(-4823, buf);
    printf("%s\n", buf);
    itoa_rec(907, buf);
    printf("%s\n", buf);
    return 0;
}
