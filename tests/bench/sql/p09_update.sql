UPDATE main_t SET v=v+1 WHERE k%7=0;
UPDATE main_t SET s=s||'x' WHERE id%1000=0;
SELECT count(*), sum(v) FROM main_t;
