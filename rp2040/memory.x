/* SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 */
/* RP2040: 2 MiB XIP flash at 0x10000000, 256-byte second-stage bootloader
   first, then the image. 256 KiB main SRAM; the USB controller has its own
   4 KiB DPRAM at 0x50100000 and is not part of this region. */
MEMORY
{
  BOOT2 : ORIGIN = 0x10000000, LENGTH = 0x100
  FLASH : ORIGIN = 0x10000100, LENGTH = 2048K - 0x100
  RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
