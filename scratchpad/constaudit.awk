# Side-II exhaustion audit of constant materialization on an AArch64 .s
function val(s){ gsub(/[# ]/,"",s); return s+0 }
function fits_add(v){ v=(v<0?-v:v); return (v<=4095) || (v%4096==0 && v/4096<=4095) }
function classify(L, N,   a,b,d,dn,imm,op,uses){
  split(L,a,/[ \t]+/); d=a[3]; sub(/,$/,"",d); imm=val(a[4]); dn=d; sub(/^[wx]/,"",dn)
  total++
  if(imm==0){ zero++; return }
  uses = (N ~ ("[wx]"dn"([, ]|$)"))
  split(N,b,/[ \t]+/); op=b[1]; sub(/^\t/,"",op)
  if(uses && (op=="add"||op=="sub"||op=="cmp"||op=="cmn") && fits_add(imm)){ foldadd++; return }
  if(uses && (op=="and"||op=="orr"||op=="eor"||op=="tst")){ foldlog++; return }
  if(uses && (op=="str"||op=="strb"||op=="strh"||op=="stur")){ storez++; return }
  if(imm>=-65536 && imm<=65535) small_remat++; else large++
}
NR>1 && prev ~ /^\tmov [wx][0-9]+,[ \t]*#-?[0-9]/ { classify(prev,$0) }
{ prev=$0 }
END{
  if(prev ~ /^\tmov [wx][0-9]+,[ \t]*#-?[0-9]/) classify(prev,"")
  printf "mov#imm total=%d\n",total
  printf "  zero -> XZR (free):         %d\n",zero
  printf "  foldable add/sub/cmp imm12: %d\n",foldadd
  printf "  foldable logical bitmask:   %d\n",foldlog
  printf "  nonzero store (str):        %d\n",storez
  printf "  small remat/multi-use:      %d\n",small_remat
  printf "  large (movz/movk needed):   %d\n",large
}
