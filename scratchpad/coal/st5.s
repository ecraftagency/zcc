.cfi_sections .eh_frame
.text
.globl f
.type f, %function
.p2align 2
f:
	.cfi_startproc
	stp x29, x30, [sp, #-16]!
	.cfi_def_cfa_offset 16
	.cfi_offset 29, -16
	.cfi_offset 30, -8
	mov x29, sp
	.cfi_def_cfa_register 29
	sub sp, sp, #32
	sub sp, sp, #16
	sub x9, x29, #48
	str x19, [x9, #0]
.Lir_f_0:
	sxtw x12, w0
	add w19, w12, #1
	add w15, w12, #2
	add w14, w12, #3
	mov x11, #0
	mov x10, #0
.Lir_f_1:
	cmp w11, w12
	b.ge .Lir_f_3
.Lir_f_2:
	add w10, w10, w12
	add w10, w10, w19
	add w10, w10, w15
	add w10, w10, w14
	add w11, w11, #1
	b .Lir_f_1
.Lir_f_3:
	mov x0, x10
	sub x9, x29, #48
	ldr x19, [x9, #0]
	mov sp, x29
	ldp x29, x30, [sp], #16
	ret
	.cfi_endproc
	.size f, .-f
