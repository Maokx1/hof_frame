# Hall of Fame frame

The Hall of Fame frame displays your photo on a color e-ink screen. To change
the photo, use a phone or computer to connect directly to the frame.

## Put a photo on the frame

1. **Get the Wi-Fi password.** Ask the person who introduced you to the frame
   for its password.
2. **Turn on the hotspot.** Press and hold the frame's button (KEY0) for
   three seconds, then release it.
3. **Connect to Wi-Fi.** On your phone or computer, join **HOF-Frame** using
   the password. Stay connected even if your device says there is no internet.
4. **Open the upload page.** In your browser, visit
   [http://bursztynki.local](http://bursztynki.local). If that address does not
   open, try [http://192.168.4.1](http://192.168.4.1). You do not need an
   internet connection.
5. **Choose and send a photo.** Select a PNG or JPEG up to 7 MB, choose the
   orientation and fit you want, and press the upload button. The page starts
   in Polish; use the language selector at the top to switch to English.
6. **Wait and check the frame.** Keep the frame powered and your device
   connected to its Wi-Fi while the page processes the photo and updates the
   display. When the page reports success, check that your photo appears on the
   frame. You can then return to your usual Wi-Fi.

The hotspot closes if an upload is not started within five minutes. If it
closes before you start, hold the button for three seconds again. If the
connection is lost during an upload, check the frame before trying again: the
outcome may be unknown.

## Documentation

- [Project plan](docs/PLAN.md) — hardware, design, and intended behavior.
- [Wiring and bill of materials](docs/WIRING.md) — connections, power safety,
  bring-up, and KiCad model.
- [Firmware guide](firmware/README.md) — flashing, uploads, troubleshooting,
  and development.
- [Firmware state machine](firmware/STATE_MACHINE.md) — button gestures,
  upload flow, and recovery.
- [Parametric Onshape enclosure](docs/enclosure/hof_frame.fs) — frame design.
