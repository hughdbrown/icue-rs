# XENEON EDGE hardware facts (M0)

Captured 2026-10-13 on a MacBook Pro (M3 Max, macOS 27) with the Edge attached by USB-C, first through
an iVANKY "20 in 1 USB-C Dock" and then directly. Commands: `system_profiler SPDisplaysDataType`,
`ioreg -p IOUSB -l -w0`, `hidutil list`, `ioreg -r -c AppleUserUSBHostHIDDevice -l -w0`, a
CoreGraphics mode dump (`CGDisplayCopyAllDisplayModes`), and `displayplacer list`.

## Summary

| Question (spec §12 M0) | Answer |
|---|---|
| USB identity of the Edge | **Two separate USB devices** behind the Edge's internal USB 2.0 hub: `CORSAIR XENEON EDGE` **0x1b1c:0x1d0d** (vendor control) and `wch.cn TouchScreen` **0x27c0:0x0859** (touch). |
| Touch report format | Multi-touch digitizer (usage page 0x0D / usage 0x04), report ID **0x0D**, **5 contacts**, each with tip switch, 4-bit contact id, X 0..16383 and Y 0..9599. It also exposes an **absolute mouse** (page 0x01 / usage 0x02, report ID 0x07) over the same coordinate space. macOS binds `AppleUserHIDEventDriver` to both, so it treats the panel as a pointer. |
| Control channel for brightness and rotation | **Yes, probably.** The Corsair device has one vendor collection (page **0xFF1B**, usage **0x91**) with 63-byte input and output reports (ID 0x01). The protocol is unknown and needs a capture of Windows iCUE (future work). |
| EDID | Manufacturer `CRX`, product **0xED00**, name `XENEON EDGE`, 36×11 cm. **Preferred timing 2560×720 @ 60.27 Hz** (138 MHz pixel clock). The CTA extension lists VICs 16, 4, 3, 2, 1, 31, 18, 19, 17 and a 1280×720 DTD. |
| macOS mode selection | macOS **does not offer native 2560×720 as a usable 1× mode**. It is listed only as HiDPI `1280×360 → 2560×720 (usable=false)`, and `CGConfigureDisplayWithDisplayMode` rejects it (`kCGErrorFailure`, 1001). The default was **1920×1080 scaled** by the monitor, which distorts the image. The private-API tool **`displayplacer` mode 29 = 2560×720 @ 60 Hz 1×** works and **persists across reboots**. |
| Through the iVANKY dock | The dock's video path drops the EDID. macOS shows `Non-PnP`, `MaxW = 1920`, a 148.5 MHz pixel-clock cap and 1024×768. **Not usable; connect the Edge directly.** USB (touch and control) still works through the dock. |
| Display name as seen by tao | `Monitor #60672` (= 0xED00), **not** "XENEON EDGE". Detection must use CoreGraphics vendor and model numbers (`CGDisplayVendorNumber` = 0x0e58, `CGDisplayModelNumber` = 0xed00), with size as a fallback. |
| Persistent display UUID | `72A30287-05E7-4AC7-8542-BAE7F8AA0392` (this unit, on this Mac) |
| Does macOS map touch to the Edge? | **Yes** (verified 2026-10-13 by tapping a button in `hello-edge` on the Edge; the click landed in the Edge's web view). macOS turns the panel's absolute-mouse reports into clicks on the Edge display, so no seize, Input Monitoring or calibration is needed for taps. |
| Does `kIOHIDOptionsTypeSeizeDevice` stop the cursor? | **Pending (M4 spike).** |

## Setup instructions for users (verified)

```bash
brew install displayplacer
displayplacer list            # find the Edge: "Type: 15 inch external screen", look for res:2560x720 hz:60
displayplacer "id:<persistent-screen-id> mode:<n>"   # the mode whose res is 2560x720, scaling off
```

Connect the Edge **directly** to the Mac (USB-C with DP Alt Mode, or HDMI), not through a dock that
cannot pass its EDID.

## EdgeHost implications

1. **Display detection:** match vendor 0x0e58 and model 0xed00 first, then the 2560×720 / 720×2560 native
   size, then the manual `--display` pin. If the Edge is found in a non-native mode, warn and print the
   `displayplacer` hint instead of silently using a stand-in window.
2. **Touch (M4):** taps already arrive as clicks on the Edge. What remains is (a) swipe and multi-touch
   gestures, since only single-pointer mouse events arrive today, and (b) cursor hygiene: hide the cursor
   or warp it back after an Edge tap. Seizing the digitizer (Input Monitoring) is needed only if M4
   wants true multi-touch.
3. **Control channel:** 0x1b1c:0x1d0d can be opened from user space with IOHIDManager (no driver). Its
   commands are unknown; treat it as a future, optional feature.

## Raw data

### EDID (256 bytes)

```text
00ffffffffffff000e5800ed010101011715010380240b78ea1ec5ae4f34b126
0e505420090001010101010101010101010101010101e83500a6a0d078203020
85006c711000001a000000fc0058454e454f4e20454447450a20000000ff0031
313038323633343536393800000000fd00384c1e5122000a2020202020200178
020318814990040302011f12131165030c001000e3050000011d007251d01e20
6e2855006c711000001e00000000000000000000000000000000000000000000
0000000000000000000000000000000000000000000000000000000000000000
00000000000000000000000000000000000000000000000000000000000000d9
```

### HID report descriptors

**XENEON EDGE vid=0x1b1c pid=0x1d0d usagePage=0xff1b usage=0x91**

```text
061bff0991a101850119002aff00150026ff007508953f8102190029ff9102c0
```

**TouchScreen vid=0x27c0 pid=0x0859 usagePage=0x000d usage=0x04**

```text
050d0904a101850d050d0922a102094215002501750195018102950381010951
7504250f95018102050126ff3f7510550e6511093035004679088102267f2546
8a0309318102050dc00922a10209421500250175019501810295038101095175
04250f95018102050126ff3f7510550e6511093035004679088102267f25468a
0309318102050dc00922a1020942150025017501950181029503810109517504
250f95018102050126ff3f7510550e6511093035004679088102267f25468a03
09318102050dc00922a102094215002501750195018102950381010951750425
0f95018102050126ff3f7510550e6511093035004679088102267f25468a0309
318102050dc00922a1020942150025017501950181029503810109517504250f
95018102050126ff3f7510550e6511093035004679088102267f25468a030931
8102050dc00922a1020942150025017501950181029503810109517504250f95
018102050126ff3f7510550e6511093035004679088102267f25468a03093181
02050dc00922a1020942150025017501950181029503810109517504250f9501
8102050126ff3f7510550e6511093035004679088102267f25468a0309318102
050dc00922a1020942150025017501950181029503810109517504250f950181
02050126ff3f7510550e6511093035004679088102267f25468a030931810205
0dc00922a1020942150025017501950181029503810109517504250f95018102
050126ff3f7510550e6511093035004679088102267f25468a0309318102050d
c00922a1020942150025017501950181029503810109517504250f9501810205
0126ff3f7510550e6511093035004679088102267f25468a0309318102050dc0
27ffff0000751095010956810209541500257f950175088102850a0955950125
0fb102c0050d090ea10185210922a100095209531500250a75089502b102c0c0
```

**TouchScreen vid=0x27c0 pid=0x0859 usagePage=0xff0a usage=0xff**

```text
060aff09ffa101855009ff15002600ff7508953f8106855109ff15002600ff75
08953f9106c0
```

**TouchScreen vid=0x27c0 pid=0x0859 usagePage=0x0001 usage=0x02**

```text
05010902a10185070901a1000509190129031500250175019503810275019505
81030501093075109501550e6511350046790826ff3f81020931468a03267f25
810209381581257f750895018106c0c0
```

