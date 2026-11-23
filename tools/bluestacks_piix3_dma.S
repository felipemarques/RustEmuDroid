/* SeaBIOS option ROM that remaps the QEMU PIIX3 IDE bus-master registers.
 * The probe assigns the controller to 00:02.0. Check its PCI ID before writes.
 * The BIOS maps this compatibility-mode controller before running the ROM.
 */
.code16
.section .text
.byte 0x55, 0xaa, 1
    pushfl
    pushal

    movw $0xcf8, %dx
    movl $0x80001000, %eax
    outl %eax, %dx
    movw $0xcfc, %dx
    inl %dx, %eax
    cmpl $0x70108086, %eax
    jne done

    /* BAR4 at configuration offset 0x20, with bit 0 denoting I/O space. */
    movw $0xcf8, %dx
    movl $0x80001020, %eax
    outl %eax, %dx
    movw $0xcfc, %dx
    movl $0xd041, %eax
    outl %eax, %dx

    /* Set the I/O-space and bus-master enable bits in the PCI command word. */
    movw $0xcf8, %dx
    movl $0x80001004, %eax
    outl %eax, %dx
    movw $0xcfc, %dx
    inw %dx, %ax
    orw $5, %ax
    outw %ax, %dx

    /* Record that the ROM reached the controller. QMP verifies the BAR. */
    movw $0xe9, %dx
    movb $'R', %al
    outb %al, %dx
done:
    popal
    popfl
    lret

/* The build step computes the option ROM checksum in the last byte. */
.org 512
