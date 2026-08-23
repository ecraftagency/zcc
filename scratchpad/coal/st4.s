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
	sub sp, sp, #48
	sub x9, x29, #112
	stp x22, x23, [x9]
	stp x21, x20, [x9, #16]
	stp x19, x24, [x9, #32]
.Lir_sum_0:
	sub x22, x29, #40
	mov x0, #1
	str w0, [x22]
	add x23, x22, #4
	mov x0, #2
	str w0, [x23]
	mov x0, #3
	str w0, [x22, #8]
	mov x0, #4
	str w0, [x22, #12]
	add x10, x22, #16
	mov x0, #5
	str w0, [x10]
	add x21, x10, #4
	mov x0, #6
	str w0, [x21]
	add x20, x10, #8
	mov x0, #7
	str w0, [x20]
	add x19, x10, #12
	mov x0, #8
	str w0, [x19]
	add x15, x22, #32
	mov x0, #99
	mov x1, x15
	str x0, [x15]
	mov x11, #0
	mov x10, #0
.Lir_sum_1:
	cmp w11, #10
	b.ge .Lir_sum_3
.Lir_sum_2:
	ldrsw x24, [x19]
	ldrsw x13, [x20]
	ldrsw x12, [x23]
	ldrsw x14, [x22]
	add w12, w14, w12
	add w12, w12, w13
	add w12, w12, w24
	add w10, w10, w12
	ldr x13, [x15]
	sxtw x12, w14
	add x12, x12, x13
	sxtw x12, w12
	mov x1, x22
	str w12, [x22]
	add w11, w11, #1
	b .Lir_sum_1
.Lir_sum_3:
	ldrsw x12, [x21]
	ldrsw x11, [x22]
	add w10, w10, w11
	add w10, w10, w12
	mov x0, x10
	sub x9, x29, #112
	ldp x22, x23, [x9]
	ldp x21, x20, [x9, #16]
	ldp x19, x24, [x9, #32]
	mov sp, x29
	ldp x29, x30, [sp], #16
	ret
	.cfi_endproc
	.size sum, .-sum
