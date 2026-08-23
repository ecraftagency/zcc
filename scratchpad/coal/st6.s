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
	sub sp, sp, #48
.Lir_sum_0:
	mov x12, #0
	mov x11, #1
	mov x10, #0
.Lir_sum_1:
	cmp w12, #10
	b.ge .Lir_sum_3
.Lir_sum_2:
	add w13, w11, #2
	add w13, w13, #7
	add w13, w13, #8
	add w10, w10, w13
	sxtw x11, w11
	add x11, x11, #99
	sxtw x11, w11
	add w12, w12, #1
	b .Lir_sum_1
.Lir_sum_3:
	add w10, w10, w11
	add w10, w10, #6
	mov x0, x10
	mov sp, x29
	ldp x29, x30, [sp], #16
	ret
	.cfi_endproc
	.size sum, .-sum
