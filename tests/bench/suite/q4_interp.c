/* q4_interp — A TINY BYTECODE INTERPRETER with a stack.
 * WHY: k1_dispatch is a switch with tiny arms and no state; a real interpreter
 * carries a stack pointer, a program counter and an accumulator across every
 * arm, so all three are loop-carried through a many-way branch — the case the
 * allocator finds hardest and the one sqlite is made of. */
#include <stdio.h>
enum { PUSH, ADD, MUL, DUP, SWAP, XOR, DROP, SHR, NEG, HALT };
static unsigned char prog[64];
static long st[64];
int main(void){
    long i, r, out = 0;
    for(i=0;i<60;i++) prog[i] = (unsigned char)((i*7) % 9);
    prog[60] = HALT;
    for(r=0;r<400000;r++){
        long sp = 0, pc = 0, acc = r;
        for(;;){
            int op = prog[pc++];
            if(op == HALT) break;
            switch(op){
                case PUSH: if(sp < 60) st[sp++] = acc + pc; break;
                case ADD:  if(sp >= 2){ st[sp-2] += st[sp-1]; sp--; } break;
                case MUL:  if(sp >= 2){ st[sp-2] = st[sp-2]*3 + st[sp-1]; sp--; } break;
                case DUP:  if(sp >= 1 && sp < 60){ st[sp] = st[sp-1]; sp++; } break;
                case SWAP: if(sp >= 2){ long t = st[sp-1]; st[sp-1] = st[sp-2]; st[sp-2] = t; } break;
                case XOR:  if(sp >= 2){ st[sp-2] ^= st[sp-1]; sp--; } break;
                case DROP: if(sp >= 1) sp--; break;
                case SHR:  if(sp >= 1) st[sp-1] >>= 1; break;
                case NEG:  if(sp >= 1) st[sp-1] = -st[sp-1]; break;
            }
            acc = sp ? st[sp-1] : acc;
        }
        out += acc & 0xffff;
    }
    printf("%ld\n", out);
    return 0;
}
