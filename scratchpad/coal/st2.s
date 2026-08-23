.cfi_sections .eh_frame
.text
.globl sum
.type sum, %function
.p2align 2
sum:
	.cfi_startproc
	stp x29, x30, [sp, #-16]!
	.cfi_def_cfa_offset 16
	.cfi_offset 29, -16
	.cfi_offset 30, -8
	mov x29, sp
	.cfi_def_cfa_register 29
	sub sp, sp, #64
.Lir_sum_0:
	add x14, sp, #24
	mov x0, #1
	mov x1, x14
	str w0, [x14]
	mov x11, #0
	mov x10, #0
.Lir_sum_1:
	cmp w11, #10
	b.ge .Lir_sum_3
.Lir_sum_2:
	ldrsw x13, [x14]
	add w12, w13, #2
	add w12, w12, #7
	add w12, w12, #8
	add w10, w10, w12
	sxtw x12, w13
	add x12, x12, #99
	sxtw x12, w12
	mov x1, x14
	str w12, [x14]
	add w11, w11, #1
	b .Lir_sum_1
.Lir_sum_3:
	ldrsw x11, [x14]
	add w10, w10, w11
	add w10, w10, #6
	mov x0, x10
	mov sp, x29
	ldp x29, x30, [sp], #16
	ret
	.cfi_endproc
	.size sum, .-sum
