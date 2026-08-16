/* K&R 3.3 + ex3-1: binary search, ban 2 diem test / 1 diem test */
#include <stdio.h>
int binsearch(int x, int v[], int n) {
    int low, high, mid;
    low = 0;
    high = n - 1;
    while (low <= high) {
        mid = (low + high) / 2;
        if (x < v[mid])
            high = mid - 1;
        else if (x > v[mid])
            low = mid + 1;
        else
            return mid;
    }
    return -1;
}
int binsearch1(int x, int v[], int n) {
    int low, high, mid;
    low = 0;
    high = n - 1;
    while (low < high) {
        mid = (low + high) / 2;
        if (x <= v[mid])
            high = mid;
        else
            low = mid + 1;
    }
    return (low == high && v[low] == x) ? low : -1;
}
int main(void) {
    int v[10] = {2, 3, 5, 7, 11, 13, 17, 19, 23, 29};
    int i;
    for (i = 0; i < 10; i++)
        if (binsearch(v[i], v, 10) != i || binsearch1(v[i], v, 10) != i) {
            printf("hong tai %d\n", i);
            return 1;
        }
    printf("%d %d %d %d\n", binsearch(4, v, 10), binsearch1(4, v, 10), binsearch(29, v, 10),
           binsearch(2, v, 10));
    return 0;
}
