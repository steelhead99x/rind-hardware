/* SPDX-License-Identifier: Apache-2.0 OR ISC OR MIT-0 */
/* EFR32MG24B220F1536IM48 (Seeed XIAO MG24): 1536 KB flash at 0x08000000,
   256 KB RAM at 0x20000000. No bootloader: the board's SAMD11 CMSIS-DAP
   probe programs the whole flash over SWD. */
MEMORY
{
  FLASH : ORIGIN = 0x08000000, LENGTH = 1536K
  RAM   : ORIGIN = 0x20000000, LENGTH = 256K
}
