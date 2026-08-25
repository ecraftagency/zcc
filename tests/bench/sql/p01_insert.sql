PRAGMA journal_mode=MEMORY;
PRAGMA synchronous=OFF;
CREATE TABLE main_t(id INTEGER PRIMARY KEY, k INTEGER, v INTEGER, s TEXT);
INSERT INTO main_t(id,k,v,s)
  WITH RECURSIVE c(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM c WHERE i<100000)
  SELECT i, (i*2654435761)%1000, (i*40503)%65536, 'row'||(i%977) FROM c;
SELECT count(*), sum(v), min(k), max(k) FROM main_t;
