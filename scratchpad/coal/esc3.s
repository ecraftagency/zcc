.cfi_sections .eh_frame
.text
.globl bump
.type bump, %function
.p2align 2
bump:
	.cfi_startproc
	stp x29, x30, [sp, #-16]!
	.cfi_def_cfa_offset 16
	.cfi_offset 29, -16
	.cfi_offset 30, -8
	mov x29, sp
	.cfi_def_cfa_register 29
	sub sp, sp, #32
.Lir_bump_0:
	add x12, x0, #4
	ldrsw x11, [x12]
	add w11, w11, #100
	str w11, [x12]
	add x11, x0, #8
	ldrsw x10, [x11]
	add w10, w10, #20
	mov x1, x11
	str w10, [x11]
	mov x0, #0
	mov sp, x29
	ldp x29, x30, [sp], #16
	ret
	.cfi_endproc
	.size bump, .-bump
.globl main
.type main, %function
.p2align 2
main:
	.cfi_startproc
	stp x29, x30, [sp, #-16]!
	.cfi_def_cfa_offset 16
	.cfi_offset 29, -16
	.cfi_offset 30, -8
	mov x29, sp
	.cfi_def_cfa_register 29
	sub sp, sp, #48
.Lir_main_0:
	sub x13, x29, #12
	mov x0, #1
	str w0, [x13]
	mov x0, #2
	str w0, [sp, #40]
	mov x0, #3
	str w0, [sp, #44]
	mov x12, x13
	add x11, x13, #4
	ldrsw x10, [x11]
	add w10, w10, #100
	str w10, [x11]
	add x11, x13, #8
	ldrsw x10, [x11]
	add w10, w10, #20
	mov x1, x11
	str w10, [x11]
	ldrsw x10, [x13]
	add w10, w10, #2
	add w10, w10, #3
	mov x0, x10
	mov sp, x29
	ldp x29, x30, [sp], #16
	ret
	.cfi_endproc
	.size main, .-main
