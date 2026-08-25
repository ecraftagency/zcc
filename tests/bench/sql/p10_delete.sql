DELETE FROM main_t WHERE k%13=0;
SELECT count(*), sum(v) FROM main_t;
PRAGMA integrity_check;
