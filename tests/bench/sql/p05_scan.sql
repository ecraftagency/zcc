SELECT count(*), sum(v), avg(v) FROM main_t WHERE v>30000;
SELECT sum(v*k) FROM main_t;
SELECT count(DISTINCT s) FROM main_t;
