/* K&R 5.7-5.8: mang 2 chieu day_of_year/month_day, con tro vao hang */
#include <stdio.h>
static char daytab[2][13] = {
    {0, 31, 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31},
    {0, 31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31}
};
int day_of_year(int year, int month, int day) {
    int i, leap;
    leap = year % 4 == 0 && year % 100 != 0 || year % 400 == 0;
    for (i = 1; i < month; i++) day += daytab[leap][i];
    return day;
}
void month_day(int year, int yearday, int *pmonth, int *pday) {
    int i, leap;
    leap = year % 4 == 0 && year % 100 != 0 || year % 400 == 0;
    for (i = 1; yearday > daytab[leap][i]; i++) yearday -= daytab[leap][i];
    *pmonth = i;
    *pday = yearday;
}
int main(void) {
    int m, d;
    printf("%d %d %d\n", day_of_year(2024, 3, 1), day_of_year(2023, 3, 1),
           day_of_year(2000, 12, 31));
    month_day(2024, 61, &m, &d);
    printf("%d/%d\n", m, d);
    month_day(1900, 60, &m, &d);
    printf("%d/%d\n", m, d);
    return 0;
}
