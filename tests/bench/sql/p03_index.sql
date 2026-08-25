CREATE INDEX i_main_k ON main_t(k);
CREATE INDEX i_main_v ON main_t(v,k);
CREATE INDEX i_side_k ON side_t(k);
ANALYZE;
SELECT count(*) FROM sqlite_master;
