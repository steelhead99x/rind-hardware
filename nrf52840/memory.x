/* SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 */
/* nRF52840 with Seeed's T1000-E layout: MBR and the S140 7.3.0 SoftDevice
   below 0x27000, the application from 0x27000, and from 0xED000 Meshtastic's
   LittleFS (its config and keys) and the UF2 bootloader at 0xF4000, which
   this image must never reach. The SoftDevice is never enabled, so RAM is
   ours but for the first KB, left alone for the MBR. */
MEMORY
{
  FLASH : ORIGIN = 0x00027000, LENGTH = 0xED000 - 0x27000
  RAM   : ORIGIN = 0x20000400, LENGTH = 256K - 1K
}
