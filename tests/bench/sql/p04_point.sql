SELECT sum(v) FROM main_t WHERE id IN (SELECT id*7 FROM side_t WHERE id<8000);
SELECT count(*) FROM main_t WHERE k=137;
SELECT sum(v) FROM main_t WHERE k BETWEEN 100 AND 120;
