CREATE TABLE e(a,b,c);
INSERT INTO e VALUES(NULL,1,'ok'),(2,NULL,''),(NULL,NULL,NULL);
SELECT count(a), count(b), count(*), total(a), coalesce(max(c),'<null>') FROM e;
SELECT length(zeroblob(2000000)), typeof(zeroblob(10)), hex(x'00ff10');
SELECT 9223372036854775807+0, -9223372036854775808+0, 9223372036854775807/1;
SELECT length('héllo wörld ☃'), upper('abc'), substr('héllo',2,3);
SELECT CAST('12abc' AS INTEGER), CAST(3.99 AS INTEGER), 7/2, 7%2, -7/2, -7%2;
SELECT 1.0/3.0, 2e308, -2e308, 0.1+0.2;
