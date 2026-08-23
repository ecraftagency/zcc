# classify reg-reg movs by destination band, and count immediate materializations
function reg(x){ sub(/^[wx]/,"",x); return x+0 }
/^\tmov[ \t]+[wx][0-9]+,[ \t]*[wx][0-9]+$/{
  split($0,a,/[ \t]+/); d=a[3]; sub(/,$/,"",d); dn=reg(d)
  if(dn<=7) arg++; else if(dn>=10&&dn<=15) wide++; else if(dn>=19&&dn<=28) callee++; else other++
  rr++
}
# immediate mov: gcc "mov x0, 36" (no #) or zcc "mov x0, #36"; also movz/movk/movn
/^\tmov[ \t]+[wx][0-9]+,[ \t]*#?-?[0-9]/{mi++}
/^\t(movz|movk|movn)[ \t]/{mz++}
END{printf "regmov=%d [arg(x0-7)=%d wide(x10-15)=%d callee(x19-28)=%d other=%d]  movimm=%d movz/k/n=%d\n",rr,arg,wide,callee,other,mi,mz}
