SELECT k, count(*), sum(v) FROM main_t GROUP BY k ORDER BY k LIMIT 20;
SELECT s, count(*) FROM main_t GROUP BY s HAVING count(*)>90 ORDER BY s LIMIT 20;
