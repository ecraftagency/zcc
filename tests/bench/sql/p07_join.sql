SELECT count(*), sum(m.v+s.w) FROM main_t m JOIN side_t s ON m.k=s.k;
SELECT count(*) FROM main_t m LEFT JOIN side_t s ON m.id=s.id WHERE s.id IS NULL;
