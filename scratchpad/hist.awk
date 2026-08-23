/^\t[a-z]/{tot++}
/^\t(ldr|ldur|ldp)[ \t]+[wxdq][0-9]+,.*\[(sp|x29)/{fl++}
/^\t(str|stur|stp)[ \t]+[wxdq][0-9]+,.*\[(sp|x29)/{fs++}
/^\tmov[ \t]+[wx][0-9]+,[ \t]*[wx][0-9]+$/{rr++}
/^\tmov[ \t]+[wx][0-9]+,[ \t]*#/{mi++}
/^\tbl[ \t]/{bl++}
END{printf "total=%d frameLD=%d frameST=%d regmov=%d movimm=%d bl=%d\n",tot,fl,fs,rr,mi,bl}
