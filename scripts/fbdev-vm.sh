#!/bin/sh
# Run the fbdev backend against a framebuffer the kernel configured.
#
#   scripts/fbdev-vm.sh [vga-mode]
#
# A framebuffer's pixel format comes from hardware, so unit tests can only
# assert constants someone typed. QEMU supplies real ones. Modes worth
# running:
#
#   0x314  800x600 RGB565    0x317  1024x768 RGB565
#   0x315  800x600 24bpp     0x303  800x600 8bpp palette (must be refused)
#
# Needs qemu-system-x86_64, busybox, cpio, and a readable host kernel.
# Nothing here needs root, and it never touches your own framebuffer.
set -e

MODE=${1:-0x314}
KERNEL=${KERNEL:-/boot/vmlinuz-$(uname -r)}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

[ -r "$KERNEL" ] || { echo "cannot read $KERNEL — set KERNEL=" >&2; exit 1; }

cargo build -p chapbook-panel-fbdev --example selftest --example probe

R=$WORK/root
mkdir -p "$R/bin" "$R/lib/x86_64-linux-gnu" "$R/lib64" "$R/proc" "$R/sys" "$R/dev"
cp target/debug/examples/selftest target/debug/examples/probe "$R/"
cp /usr/bin/busybox "$R/bin/"
ln -s busybox "$R/bin/sh"
# busybox and the examples are dynamically linked; carry just their libs.
for lib in $(ldd /usr/bin/busybox target/debug/examples/selftest |
             awk '/=> \//{print $3}' | sort -u); do
    cp "$lib" "$R/lib/x86_64-linux-gnu/"
done
cp /lib64/ld-linux-x86-64.so.2 "$R/lib64/"

cat > "$R/init" <<'INIT'
#!/bin/sh
B=/bin/busybox
$B mount -t proc none /proc
$B mount -t sysfs none /sys
# Stock kernels do not all set CONFIG_DEVTMPFS_MOUNT, so /dev is ours to make.
$B mount -t devtmpfs none /dev
echo "### fb0: $($B cat /sys/class/graphics/fb0/name 2>&1), \
$($B cat /sys/class/graphics/fb0/bits_per_pixel 2>&1) bpp, \
$($B cat /sys/class/graphics/fb0/virtual_size 2>&1)"
echo "### PROBE";    /probe /dev/fb0 2>&1
echo "### SELFTEST"; /selftest /dev/fb0 2>&1; echo "SELFTEST_EXIT=$?"
$B poweroff -f
INIT
chmod +x "$R/init"

( cd "$R" && find . | cpio -o -H newc --quiet ) | gzip -9 > "$WORK/initramfs.gz"

echo "=== booting vga=$MODE ==="
exec qemu-system-x86_64 \
    -kernel "$KERNEL" -initrd "$WORK/initramfs.gz" \
    -append "console=ttyS0 vga=$MODE nomodeset panic=1" \
    -vga std -m 512 -nographic -no-reboot
