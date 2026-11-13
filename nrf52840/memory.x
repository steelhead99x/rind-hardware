/* SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 */
/* nRF52840 on the Arduino Nano 33 BLE with its stock BOSSA bootloader.
   The bootloader occupies 0x00000-0x10000 and must never be written.
   The application starts at 0x10000, which is where the bootloader jumps
   and where `bossac --offset 0x10000` programs the image. There is no
   SoftDevice and no trailing UF2 bootloader (this is not the T1000-E
   layout). RAM skips the first 512 bytes, matching ArduinoCore-mbed's
   official linker script (RAM_NVIC + RAM_CRASH_DATA). */
MEMORY
{
  FLASH : ORIGIN = 0x00010000, LENGTH = 0xF0000
  RAM   : ORIGIN = 0x20000200, LENGTH = 0x40000 - 0x200
}
